use base64::{Engine, engine::general_purpose::STANDARD};
use gaap_attestation::dsse::{self, EnvelopeSignature, Signer as _};
use gaap_attestation::ed25519::{Ed25519Signer, Ed25519Verifier};

const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
fn keys() -> (Ed25519Signer, Ed25519Verifier) {
    // Public test seed. It must never be configured as a production trust key.
    let signer = Ed25519Signer::from_seed("fixture-only".into(), [7; 32]).unwrap();
    let verifier = Ed25519Verifier::new([("fixture-only".into(), signer.public_key())]).unwrap();
    (signer, verifier)
}
#[test]
fn published_pae_uses_raw_bytes_and_byte_lengths() {
    assert_eq!(
        dsse::pre_authentication_encoding("text/plain", b"hello").unwrap(),
        b"DSSEv1 10 text/plain 5 hello"
    );
    // Non-English protocol test data: UTF-8 byte count differs from character count.
    let payload = "\u{1f642}".as_bytes();
    let message = dsse::pre_authentication_encoding("text/plain", payload).unwrap();
    assert!(message.starts_with(b"DSSEv1 10 text/plain 4 "));
    assert!(message.ends_with(payload));
}
#[test]
fn verified_bytes_stay_identical_without_json_canonicalization() {
    let (mut signer, mut verifier) = keys();
    let payload = b"{ \"second\": 2, \"first\": 1 }\n";
    let envelope = dsse::sign(PAYLOAD_TYPE, payload, &mut signer).unwrap();
    let verified = dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier).unwrap();
    assert_eq!(verified.payload(), payload);
    assert_eq!(verified.keys()[0].key_id, "fixture-only");
}
#[test]
fn payload_type_bytes_signature_and_key_tampering_fail() {
    let (mut signer, mut verifier) = keys();
    let original = dsse::sign(PAYLOAD_TYPE, b"{}", &mut signer).unwrap();
    for field in ["type", "payload", "signature", "key"] {
        let mut envelope = original.clone();
        match field {
            "type" => envelope.payload_type = "text/plain".into(),
            "payload" => envelope.payload = STANDARD.encode(b"[]"),
            "signature" => envelope.signatures[0].sig = STANDARD.encode([0u8; 64]),
            "key" => envelope.signatures[0].keyid = "untrusted".into(),
            _ => unreachable!(),
        }
        assert!(
            dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier).is_err(),
            "{field}"
        );
    }
}
#[test]
fn unsigned_input_cant_construct_verified_evidence() {
    let (mut signer, mut verifier) = keys();
    let mut envelope = dsse::sign(PAYLOAD_TYPE, b"{}", &mut signer).unwrap();
    envelope.signatures.clear();
    assert_eq!(
        dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier)
            .err()
            .unwrap()
            .code,
        "dsse.unsigned"
    );
}
#[test]
fn duplicate_key_hints_dont_create_independent_signers() {
    let (mut signer, mut verifier) = keys();
    let mut envelope = dsse::sign(PAYLOAD_TYPE, b"{}", &mut signer).unwrap();
    envelope.signatures.push(envelope.signatures[0].clone());
    assert_eq!(
        dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier)
            .err()
            .unwrap()
            .code,
        "dsse.duplicate_signer"
    );
    let mut aliases = Ed25519Verifier::new([
        ("fixture-only".into(), signer.public_key()),
        ("alias".into(), signer.public_key()),
    ])
    .unwrap();
    envelope.signatures[1].keyid = "alias".into();
    assert_eq!(
        dsse::verify(&envelope, PAYLOAD_TYPE, &mut aliases)
            .err()
            .unwrap()
            .code,
        "dsse.duplicate_signer"
    );
}
#[test]
fn distinct_trusted_signatures_can_share_an_envelope() {
    let (mut first, _) = keys();
    let mut second = Ed25519Signer::from_seed("second-fixture".into(), [8; 32]).unwrap();
    let mut envelope = dsse::sign(PAYLOAD_TYPE, b"{}", &mut first).unwrap();
    let message = dsse::pre_authentication_encoding(PAYLOAD_TYPE, b"{}").unwrap();
    envelope.signatures.push(EnvelopeSignature {
        keyid: second.key_id().into(),
        sig: STANDARD.encode(second.sign(&message).unwrap()),
    });
    let mut verifier = Ed25519Verifier::new([
        ("fixture-only".into(), first.public_key()),
        ("second-fixture".into(), second.public_key()),
    ])
    .unwrap();
    assert_eq!(
        dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier)
            .unwrap()
            .keys()
            .len(),
        2
    );
}
#[test]
fn envelope_extensions_are_ignored_but_duplicate_authentication_fields_fail() {
    let duplicate = br#"{"payloadType":"type","payloadType":"other","payload":"","signatures":[]}"#;
    assert!(dsse::parse_envelope(duplicate).is_err());
    let extended =
        br#"{"payloadType":"type","payload":"","signatures":[],"future_extension":{"value":1}}"#;
    assert!(dsse::parse_envelope(extended).is_ok());
}

#[test]
fn serde_struct_deserialization_rejects_every_duplicate_authentication_field() {
    for field in ["payloadType", "payload", "signatures"] {
        let value = if field == "signatures" {
            "[]"
        } else {
            "\"same\""
        };
        let input = format!(
            "{{\"payloadType\":\"type\",\"payload\":\"\",\"signatures\":[],\"{field}\":{value}}}"
        );
        assert!(dsse::parse_envelope(input.as_bytes()).is_err(), "{field}");
    }
    for field in ["keyid", "sig"] {
        let input = format!(
            "{{\"payloadType\":\"type\",\"payload\":\"\",\"signatures\":[{{\"keyid\":\"key\",\"sig\":\"value\",\"{field}\":\"same\"}}]}}"
        );
        assert!(dsse::parse_envelope(input.as_bytes()).is_err(), "{field}");
    }
}
#[test]
fn malformed_encodings_and_payload_bounds_fail_closed() {
    let (mut signer, mut verifier) = keys();
    let mut envelope = dsse::sign(PAYLOAD_TYPE, b"{}", &mut signer).unwrap();
    envelope.payload = "%%%".into();
    assert!(dsse::verify(&envelope, PAYLOAD_TYPE, &mut verifier).is_err());
    assert!(
        dsse::pre_authentication_encoding(PAYLOAD_TYPE, &vec![0; dsse::MAX_PAYLOAD_BYTES + 1])
            .is_err()
    );
    assert!(dsse::parse_envelope(&vec![b' '; dsse::MAX_ENVELOPE_BYTES + 1]).is_err());
}
#[test]
fn node_independently_verifies_the_rust_signature() {
    let (mut signer, _) = keys();
    let payload = b"{\"fixture\":true}";
    let envelope = dsse::sign(PAYLOAD_TYPE, payload, &mut signer).unwrap();
    let public = STANDARD.encode(signer.public_key());
    let result=std::process::Command::new("node").args(["-e",r#"
const crypto = require('node:crypto');
const envelope = JSON.parse(process.argv[1]);
const payload = Buffer.from(envelope.payload,'base64');
const prefix = Buffer.from(`DSSEv1 ${Buffer.byteLength(envelope.payloadType)} ${envelope.payloadType} ${payload.length} `);
const key = crypto.createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(process.argv[2],'base64')]),format:'der',type:'spki'});
if (!crypto.verify(null,Buffer.concat([prefix,payload]),key,Buffer.from(envelope.signatures[0].sig,'base64'))) process.exit(1);
"#,&serde_json::to_string(&envelope).unwrap(),&public]).status().unwrap();
    assert!(result.success());
}
