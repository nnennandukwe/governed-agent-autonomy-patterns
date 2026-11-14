//! Explicitly pinned Cedar static authorization for GAAP's protected effects.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use cedar_policy::{
    ActionConstraint, Authorizer, Context, Decision, Effect, Entities, EntityUid, PolicySet,
    PrincipalConstraint, Request, ResourceConstraint, Schema, ValidationMode, Validator,
};
use gaap::contracts::{
    CapabilityIdentity, OperationFamily, PolicyIdentity, ProtectedEffectRequest, RequestedScope,
    canonical_protected_effect_request_bytes,
};
use gaap::runtime::PolicyDecision;
use gaap::static_policy::{StaticPolicyAdapter, StaticPolicyEvaluation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Adapter failures require corrected local inputs and explicit revalidation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError(pub String);
impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; correct the local pack or context, validate and pin it again",
            self.0
        )
    }
}
impl Error for PolicyError {}
fn error(value: impl fmt::Display) -> PolicyError {
    PolicyError(value.to_string())
}

/// An operation approved by the host. Protocol/SDK types stay in their own adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedAction {
    pub operation_family: OperationFamily,
    pub normalized_operation: String,
    pub capability: CapabilityIdentity,
    pub tool_schema_digest: Option<String>,
    pub mcp: Option<McpDescriptor>,
}

/// The exact approved MCP descriptor and server identity, without SDK types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpDescriptor {
    pub server_digest: String,
    pub protocol: String,
    pub descriptor: Value,
}

impl ApprovedAction {
    pub fn native(
        family: OperationFamily,
        operation: String,
        capability: CapabilityIdentity,
    ) -> Self {
        Self {
            operation_family: family,
            normalized_operation: operation,
            capability,
            tool_schema_digest: None,
            mcp: None,
        }
    }

    /// Use the same identities as GAAP's outbound MCP discovery adapter.
    pub fn mcp(
        server_digest: String,
        protocol: String,
        descriptor: Value,
    ) -> Result<Self, PolicyError> {
        if bounded_json(&descriptor)? > 1024 * 1024 {
            return Err(error("MCP descriptor exceeds its byte bound"));
        }
        let name = descriptor
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| error("MCP descriptor needs a name"))?;
        if !is_digest(&server_digest) || protocol.trim().is_empty() {
            return Err(error("MCP server and protocol identities are required"));
        }
        let input_schema = descriptor
            .get("inputSchema")
            .ok_or_else(|| error("MCP inputSchema is required"))?;
        if input_schema["type"] != "object" {
            return Err(error("MCP inputSchema must describe an object"));
        }
        cedar_type(input_schema, 0)?;
        let capability = CapabilityIdentity {
            name: format!("mcp.{name}"),
            version: protocol.clone(),
            digest: content_digest(&json!({"domain":"gaap.mcp-capability/0.1",
                "server":server_digest,"protocol":protocol,"tool":descriptor}))?,
        };
        Ok(Self {
            operation_family: OperationFamily::ExternalService,
            normalized_operation: "external_service.mcp_call".into(),
            capability,
            tool_schema_digest: Some(content_digest(input_schema)?),
            mcp: Some(McpDescriptor {
                server_digest,
                protocol,
                descriptor,
            }),
        })
    }

    fn validate(&self) -> Result<(), PolicyError> {
        if self.capability.name.trim().is_empty()
            || self.capability.version.trim().is_empty()
            || !is_digest(&self.capability.digest)
            || !self
                .normalized_operation
                .starts_with(&format!("{}.", family_name(self.operation_family)))
            || self
                .normalized_operation
                .split_once('.')
                .is_none_or(|(_, op)| op.is_empty())
        {
            return Err(error("invalid approved operation or capability identity"));
        }
        if let Some(mcp) = &self.mcp {
            if Self::mcp(
                mcp.server_digest.clone(),
                mcp.protocol.clone(),
                mcp.descriptor.clone(),
            )? != *self
            {
                return Err(error(
                    "MCP descriptor identities don't match the approved capability",
                ));
            }
        } else if self.tool_schema_digest.is_some()
            || self.normalized_operation == "external_service.mcp_call"
        {
            return Err(error("MCP calls require an approved descriptor"));
        }
        Ok(())
    }
}

/// Validate and content-address a local pack before host activation.
#[derive(Debug)]
pub struct PreparedPack {
    principal: String,
    policies: PolicySet,
    schema: Schema,
    schema_document: Value,
    actions: Vec<ApprovedAction>,
    identities: [PolicyIdentity; 2],
}

impl PreparedPack {
    pub fn prepare(
        name: &str,
        version: &str,
        principal: &str,
        source: &str,
        mut actions: Vec<ApprovedAction>,
    ) -> Result<Self, PolicyError> {
        if [name, version, principal, source]
            .iter()
            .any(|component| component.trim().is_empty())
            || source.len() > 1024 * 1024
            || actions.is_empty()
            || actions.len() > 128
        {
            return Err(error(
                "a bounded nonempty local policy pack and action inventory are required",
            ));
        }
        if serde_json::to_vec(&actions).map_err(error)?.len() > 1024 * 1024 {
            return Err(error("approved action inventory exceeds its byte bound"));
        }
        actions.sort_by_key(|action| {
            action_uid(action)
                .map(|uid| uid.to_string())
                .unwrap_or_default()
        });
        let mut action_schemas = serde_json::Map::new();
        for action in &actions {
            action.validate()?;
            let mut context = common_context();
            if let Some(mcp) = &action.mcp {
                context["attributes"]["arguments"] = cedar_type(&mcp.descriptor["inputSchema"], 0)?;
            }
            let key = action_id(action)?;
            if action_schemas
                .insert(
                    key,
                    json!({"appliesTo":{
                "principalTypes":["AgentRun"], "resourceTypes":[resource_type(action.operation_family)],
                "context":context}}),
                )
                .is_some()
            {
                return Err(error("duplicate approved action"));
            }
        }
        let schema_document = json!({"Gaap":{
            "entityTypes":{
                "AgentRun":{},
                "Filesystem":{"shape":record(json!({"root":string_type(),"access":{"type":"Set","element":string_type()},"recursive":{"type":"Boolean"}}))},
                "Process":{"shape":record(json!({"executable":string_type(),"working_directory":string_type()}))},
                "Network":{"shape":record(json!({"protocol":string_type(),"host":string_type(),"port":{"type":"Long"}}))},
                "ExternalService":{"shape":record(json!({"service":string_type(),"operation":string_type(),"resource":string_type()}))}
            }, "actions":action_schemas}});
        let schema = Schema::from_json_value(schema_document.clone()).map_err(error)?;
        let policies: PolicySet = source.parse().map_err(error)?;
        if policies.policies().next().is_none() || policies.templates().next().is_some() {
            return Err(error(
                "static policies are required; templates aren't supported",
            ));
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed() {
            return Err(error(format!(
                "policy/schema incompatibility: {:?}",
                validation.validation_errors().collect::<Vec<_>>()
            )));
        }
        let expected_principal = run_uid(principal)?;
        for policy in policies
            .policies()
            .filter(|policy| policy.effect() == Effect::Permit)
        {
            if policy.principal_constraint() != PrincipalConstraint::Eq(expected_principal.clone())
                || !matches!(policy.action_constraint(), ActionConstraint::Eq(ref id) if actions.iter().any(|approved_action| action_uid(approved_action).ok().as_ref() == Some(id)))
                || !matches!(policy.resource_constraint(), ResourceConstraint::Eq(ref id) if is_digest(id.id().as_ref()))
            {
                return Err(error(
                    "permit needs the exact run, approved action and content-addressed resource; wildcards aren't supported",
                ));
            }
        }
        let identities = [
            PolicyIdentity {
                name: name.into(),
                version: version.into(),
                digest: content_digest(
                    &json!({"domain":"gaap.cedar-policy/1","name":name,"version":version,"source":source}),
                )?,
            },
            PolicyIdentity {
                name: "gaap.cedar-schema".into(),
                version: "1".into(),
                digest: content_digest(
                    &json!({"domain":"gaap.cedar-schema/1","mapping":"exact-input-scope/1","principal":principal,"actions":actions,"schema":schema_document}),
                )?,
            },
        ];
        Ok(Self {
            principal: principal.into(),
            policies,
            schema,
            schema_document,
            actions,
            identities,
        })
    }

    pub fn identities(&self) -> &[PolicyIdentity; 2] {
        &self.identities
    }
    pub fn schema_document(&self) -> &Value {
        &self.schema_document
    }

    /// Only the embedding host calls this, after approving both displayed identities.
    /// Inputs are immutable canonical preimages indexed by their exact input digest.
    pub fn activate(
        self,
        expected: &[PolicyIdentity; 2],
        inputs: BTreeMap<String, Value>,
    ) -> Result<CedarAdapter, PolicyError> {
        if expected != &self.identities {
            return Err(error("policy/schema pin mismatch"));
        }
        if inputs.len() > 128 {
            return Err(error("too many bound inputs"));
        }
        for (digest, input) in &inputs {
            if bounded_json(input)? > 1024 * 1024 || content_digest(input)? != *digest {
                return Err(error(
                    "input context doesn't match its digest or size bound",
                ));
            }
        }
        Ok(CedarAdapter { pack: self, inputs })
    }
}

/// An immutable, explicitly activated adapter. It never reads files or downloads policy.
#[derive(Debug)]
pub struct CedarAdapter {
    pack: PreparedPack,
    inputs: BTreeMap<String, Value>,
}

impl StaticPolicyAdapter for CedarAdapter {
    fn evaluate(&self, effect: &ProtectedEffectRequest) -> StaticPolicyEvaluation {
        let effect_request_digest = canonical_protected_effect_request_bytes(effect)
            .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)))
            .unwrap_or_default();
        let mut evidence = StaticPolicyEvaluation {
            effect_request_digest,
            policy: self.pack.identities[0].clone(),
            schema: self.pack.identities[1].clone(),
            decision: PolicyDecision::Deny,
            reason: "cedar.denied".into(),
            determining_policies: vec![],
            errors: vec![],
        };
        match self.evaluate_inner(effect) {
            Ok((allowed, reasons)) => {
                evidence.determining_policies = reasons;
                if allowed {
                    evidence.decision = PolicyDecision::Allow;
                    evidence.reason = "cedar.allowed".into();
                }
            }
            Err(err) => {
                evidence.reason = "cedar.invalid_context".into();
                evidence.errors.push(err.to_string());
            }
        }
        evidence
    }
}

impl CedarAdapter {
    fn evaluate_inner(
        &self,
        effect: &ProtectedEffectRequest,
    ) -> Result<(bool, Vec<String>), PolicyError> {
        if !self
            .pack
            .identities
            .iter()
            .all(|id| effect.policies.contains(id))
        {
            return Err(error(
                "effect policy/schema pins don't match the active pack",
            ));
        }
        if effect.run_id != self.pack.principal
            || effect.run_id.trim().is_empty()
            || effect.subject.locator.trim().is_empty()
            || !is_digest(&effect.subject.digest)
            || effect.requested_scopes.is_empty()
        {
            return Err(error("run, subject and exact scopes are required"));
        }
        let action = self
            .pack
            .actions
            .iter()
            .find(|action| {
                action.operation_family == effect.operation_family
                    && action.normalized_operation == effect.normalized_operation
                    && action.capability == effect.capability
                    && action.tool_schema_digest == effect.tool_schema_digest
            })
            .ok_or_else(|| error("unknown operation, capability or tool schema"))?;
        let input = self
            .inputs
            .get(&effect.input_digest)
            .ok_or_else(|| error("missing bound input context"))?;
        let mut context = json!({"run_id":effect.run_id,"subject_digest":effect.subject.digest,
            "subject_locator":effect.subject.locator,"effect_request_digest":content_digest(effect)?,
            "input_digest":effect.input_digest,"input_json":serde_jcs::to_string(input).map_err(error)?,
            "capability_digest":effect.capability.digest,"sandbox_digest":effect.sandbox_profile.digest,
            "policy_digest":self.pack.identities[0].digest,"schema_digest":self.pack.identities[1].digest});
        if let Some(mcp) = &action.mcp {
            let name = mcp.descriptor["name"]
                .as_str()
                .expect("validated tool name");
            if input
                .as_object()
                .is_none_or(|input_fields| input_fields.len() != 3)
                || input["server"] != mcp.server_digest
                || input["tool"] != name
            {
                return Err(error("MCP input doesn't bind the approved server and tool"));
            }
            context["arguments"] = input
                .get("arguments")
                .cloned()
                .ok_or_else(|| error("MCP arguments are required"))?;
            if effect.requested_scopes.iter().any(|scope| !matches!(scope, RequestedScope::ExternalService {service,operation,resource} if service == &mcp.server_digest && operation == name && resource == &effect.subject.locator)) {
                return Err(error("MCP scopes don't bind the approved server, tool and subject"));
            }
        }
        let principal = run_uid(&self.pack.principal)?;
        let action_uid = action_uid(action)?;
        let context = Context::from_json_value(context, Some((&self.pack.schema, &action_uid)))
            .map_err(error)?;
        let mut all_allowed = true;
        let mut reasons = Vec::new();
        for scope in &effect.requested_scopes {
            if scope_family(scope) != effect.operation_family {
                return Err(error("scope family doesn't match the action"));
            }
            validate_scope(scope)?;
            let resource = resource_uid(action, scope, &effect.input_digest)?;
            let mut attrs = serde_json::to_value(scope).map_err(error)?;
            attrs
                .as_object_mut()
                .expect("scope object")
                .remove("scope_type");
            let resource_id: &str = resource.id().as_ref();
            let entities = Entities::from_json_value(json!([
                {"uid":{"type":"Gaap::AgentRun","id":self.pack.principal},"attrs":{},"parents":[]},
                {"uid":{"type":format!("Gaap::{}",resource_type(effect.operation_family)),"id":resource_id},"attrs":attrs,"parents":[]}
            ]), Some(&self.pack.schema)).map_err(error)?;
            let request = Request::new(
                principal.clone(),
                action_uid.clone(),
                resource,
                context.clone(),
                Some(&self.pack.schema),
            )
            .map_err(error)?;
            let response =
                Authorizer::new().is_authorized(&request, &self.pack.policies, &entities);
            if response.diagnostics().errors().next().is_some() {
                return Err(error(format!(
                    "Cedar evaluation errors: {:?}",
                    response.diagnostics().errors().collect::<Vec<_>>()
                )));
            }
            all_allowed &= response.decision() == Decision::Allow;
            reasons.extend(response.diagnostics().reason().map(ToString::to_string));
        }
        reasons.sort();
        reasons.dedup();
        Ok((all_allowed, reasons))
    }
}

/// Render a narrow permit for one accepted Agent Run without exposing Cedar SDK types to the host.
/// Resource identity includes the complete ordered input as well as the exact scope.
pub fn exact_permit(
    principal: &str,
    action: &ApprovedAction,
    scope: &RequestedScope,
    input_digest: &str,
) -> Result<String, PolicyError> {
    action.validate()?;
    validate_scope(scope)?;
    if scope_family(scope) != action.operation_family || !is_digest(input_digest) {
        return Err(error("invalid exact permit binding"));
    }
    Ok(format!(
        "permit(principal == {}, action == {}, resource == {});",
        run_uid(principal)?,
        action_uid(action)?,
        resource_uid(action, scope, input_digest)?
    ))
}

pub fn content_digest(value: &impl Serialize) -> Result<String, PolicyError> {
    let bytes = serde_jcs::to_vec(value).map_err(error)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
fn run_uid(principal: &str) -> Result<EntityUid, PolicyError> {
    uid("AgentRun", principal)
}
fn uid(kind: &str, id: &str) -> Result<EntityUid, PolicyError> {
    format!(
        "Gaap::{kind}::{}",
        serde_json::to_string(id).map_err(error)?
    )
    .parse()
    .map_err(error)
}
fn action_id(action: &ApprovedAction) -> Result<String, PolicyError> {
    content_digest(action)
}
fn action_uid(action: &ApprovedAction) -> Result<EntityUid, PolicyError> {
    uid("Action", &action_id(action)?)
}
fn resource_uid(
    action: &ApprovedAction,
    scope: &RequestedScope,
    input_digest: &str,
) -> Result<EntityUid, PolicyError> {
    uid(
        resource_type(action.operation_family),
        &content_digest(
            &json!({"domain":"gaap.cedar-resource/1","action":action,"scope":scope,"input_digest":input_digest}),
        )?,
    )
}
fn family_name(family: OperationFamily) -> &'static str {
    match family {
        OperationFamily::Filesystem => "filesystem",
        OperationFamily::Process => "process",
        OperationFamily::Network => "network",
        OperationFamily::ExternalService => "external_service",
    }
}
fn resource_type(family: OperationFamily) -> &'static str {
    match family {
        OperationFamily::Filesystem => "Filesystem",
        OperationFamily::Process => "Process",
        OperationFamily::Network => "Network",
        OperationFamily::ExternalService => "ExternalService",
    }
}
fn scope_family(scope: &RequestedScope) -> OperationFamily {
    match scope {
        RequestedScope::Filesystem { .. } => OperationFamily::Filesystem,
        RequestedScope::Process { .. } => OperationFamily::Process,
        RequestedScope::Network { .. } => OperationFamily::Network,
        RequestedScope::ExternalService { .. } => OperationFamily::ExternalService,
    }
}
fn string_type() -> Value {
    json!({"type":"String"})
}
fn record(attrs: Value) -> Value {
    json!({"type":"Record","attributes":attrs})
}
fn common_context() -> Value {
    record(
        json!({"run_id":string_type(),"subject_digest":string_type(),"subject_locator":string_type(),"effect_request_digest":string_type(),"input_digest":string_type(),"input_json":string_type(),"capability_digest":string_type(),"sandbox_digest":string_type(),"policy_digest":string_type(),"schema_digest":string_type()}),
    )
}
fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn validate_scope(scope: &RequestedScope) -> Result<(), PolicyError> {
    fn exact(value: &str) -> bool {
        !value.trim().is_empty() && !value.contains(['*', '?', '\0'])
    }
    fn path(value: &str) -> bool {
        exact(value)
            && value.starts_with('/')
            && !value.contains("//")
            && (value == "/" || !value.ends_with('/'))
            && !value.split('/').any(|part| matches!(part, "." | ".."))
    }
    let valid = match scope {
        RequestedScope::Filesystem { root, access, .. } => path(root) && !access.is_empty(),
        RequestedScope::Process {
            executable,
            working_directory,
        } => path(executable) && path(working_directory),
        RequestedScope::Network { host, port, .. } => {
            exact(host) && !host.contains(['/', ':', '@', ' ']) && *port > 0
        }
        RequestedScope::ExternalService {
            service,
            operation,
            resource,
        } => [service, operation, resource]
            .into_iter()
            .all(|scope_component| exact(scope_component)),
    };
    if valid {
        Ok(())
    } else {
        Err(error(
            "scope must use exact normalized boundaries without wildcards",
        ))
    }
}

// A closed JSON Schema profile: unsupported semantics fail activation instead of being dropped.
fn cedar_type(schema: &Value, depth: usize) -> Result<Value, PolicyError> {
    if depth > 12 {
        return Err(error("MCP input schema is too deep"));
    }
    let schema_fields = schema
        .as_object()
        .ok_or_else(|| error("MCP schema must be an object"))?;
    if schema_fields.keys().any(|keyword| {
        ![
            "type",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "description",
            "title",
        ]
        .contains(&keyword.as_str())
    }) {
        return Err(error("unsupported MCP input schema keyword"));
    }
    Ok(match schema["type"].as_str() {
        Some("string") => string_type(),
        Some("boolean") => json!({"type":"Boolean"}),
        Some("integer") => json!({"type":"Long"}),
        Some("array") => json!({"type":"Set","element":cedar_type(&schema["items"],depth+1)?}),
        Some("object") => {
            if schema["additionalProperties"] != false {
                return Err(error("MCP objects must set additionalProperties to false"));
            }
            let properties = schema["properties"]
                .as_object()
                .ok_or_else(|| error("MCP object properties are required"))?;
            let required = schema
                .get("required")
                .map(|required_fields| {
                    required_fields
                        .as_array()
                        .ok_or_else(|| error("MCP required must be an array"))
                })
                .transpose()?
                .cloned()
                .unwrap_or_default();
            if required.iter().any(|required_field| {
                required_field
                    .as_str()
                    .is_none_or(|property_name| !properties.contains_key(property_name))
            }) {
                return Err(error("unknown required MCP property"));
            }
            let attributes: Result<serde_json::Map<String, Value>, PolicyError> = properties
                .iter()
                .map(|(name, property_schema)| {
                    let mut property_schema = cedar_type(property_schema, depth + 1)?;
                    property_schema["required"] =
                        json!(required.contains(&Value::String(name.clone())));
                    Ok((name.clone(), property_schema))
                })
                .collect();
            record(Value::Object(attributes?))
        }
        _ => return Err(error("unsupported MCP input schema type")),
    })
}
fn bounded_json(value: &Value) -> Result<usize, PolicyError> {
    fn visit(value: &Value, depth: usize) -> Result<(), PolicyError> {
        if depth > 32 {
            return Err(error("input context is too deep"));
        }
        match value {
            Value::Number(number)
                if number
                    .as_i64()
                    .is_none_or(|integer| integer.unsigned_abs() > 9_007_199_254_740_991) =>
            {
                return Err(error("inputs require JCS-safe integers"));
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, depth + 1)?;
                }
            }
            Value::Object(fields) => {
                for field in fields.values() {
                    visit(field, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, 0)?;
    Ok(serde_json::to_vec(value).map_err(error)?.len())
}
