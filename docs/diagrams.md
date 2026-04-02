# Governed Agent Autonomy Diagrams

## Control Plane Overview

This diagram shows the full five-gate control plane: planning, permission, tool trust, verification, and runtime accountability all sit between the operator and raw execution power.

![Governed autonomy control plane](./assets/diagrams/control-plane-overview.png)

In the visual set, the fifth gate is rendered as a telemetry and quota gate. In the repo taxonomy, that gate is called `runtime accountability`.

Portable mermaid version:

```mermaid
flowchart TD
  U["User Request"] --> P["Plan Gate"]
  P --> A["Approval"]
  A --> E["Execution Target"]
  E --> G["Permission Gate"]
  G --> T["Tool Trust Gate"]
  T --> I["Implementation"]
  I --> V["Independent Verification"]
  V --> C["Completion"]

  E --> R["Runtime Accountability Gate"]
  I --> R
  V --> R
  R --> O{"Threshold Crossed?"}
  O -- "No" --> C
  O -- "Yes" --> X["Alert, Ask, Or Stop"]
```

## Primitive vs Governed Autonomy

This comparison makes the core reframe explicit: the difference is not intelligence alone. It is the surrounding system that turns capability into something governable.

![Primitive autonomy vs governed autonomy](./assets/diagrams/primitive-vs-governed-autonomy.png)

## Plan Flow

This diagram shows that the planning path exists to delay mutation until the system has inspected context and a human has approved the proposed path.

```mermaid
flowchart LR
  R["Request"] --> X["Explore Context"]
  X --> N["Proposed Plan"]
  N --> H{"Explicit Approval?"}
  H -- "No" --> F["Refine or Reject"]
  F --> X
  H -- "Yes" --> E["Execute"]
```

## Permission Flow

This diagram shows why safe defaults matter: rule matching alone is not enough when dangerous operations need a harder stop.

```mermaid
flowchart LR
  Q["Tool Request"] --> M["Rule Evaluation"]
  M --> D["Dangerous Override Checks"]
  D --> O{"Outcome"}
  O -- "Allow" --> A["Run"]
  O -- "Ask" --> P["Prompt For Approval"]
  O -- "Deny" --> X["Block"]
```

## Tool Trust Flow

This diagram shows that new servers, tools, and risky settings should not become trusted just because they exist.

```mermaid
flowchart LR
  N["New Tool, Server, or Setting"] --> K["Risk Review"]
  K --> D{"Decision"}
  D -- "Approve" --> A["Enable For Use"]
  D -- "Reject" --> B["Keep Disabled"]
  D -- "Needs Revision" --> R["Revise And Re-review"]
```

## Verification Flow

This diagram shows the separation between implementation and verification. The verifier is a different role with a different job.

```mermaid
flowchart LR
  I["Implementer"] --> C["Changed Artifact"]
  C --> V["Independent Verifier"]
  V --> E["Evidence-Bearing Checks"]
  E --> R["Verdict"]
```

## Runtime Accountability Gate

This diagram shows the runtime-accountability layer around agent execution: trace context, model and tool events, usage collection, quota evaluation, and policy decisions when thresholds are crossed.

![Agent observability and quota governance](./assets/diagrams/agent-observability-and-quota-governance.png)

Supporting mermaid version:

```mermaid
flowchart TD
  P["Planning Events"] --> V["Runtime Accountability Layer"]
  G["Permission Decisions"] --> V
  T["Trust Reviews"] --> V
  R["Verification Verdicts"] --> V
  Q["Quota and Cost Signals"] --> V

  V --> L["Logs"]
  V --> N["Notifications"]
  V --> S["Execution State"]
  V --> A["Audit Trail"]
```

## Governed Publish Pipeline

This diagram shows how the same five gates apply to package release. The publish path should be reviewable, policy-bound, artifact-aware, independently verified, and operationally accountable after publish.

![Governance from code generation to release](./assets/diagrams/governance-code-generation-to-release.png)

In the release visual, `telemetry` is the runtime-accountability gate applied to shipped artifacts, provenance, and release-state visibility.

Portable mermaid version:

```mermaid
flowchart LR
  R["Request"] --> P["Plan"]
  P --> C["Code Change"]
  C --> B["Build Artifact"]
  B --> V1["Verification (Pre-publish)"]
  V1 --> U["Publish Or Deploy"]
  U --> V2["Verification (Post-publish)"]
  V2 --> A["Runtime Accountability"]
  A --> S["Shipped Artifact"]
```
