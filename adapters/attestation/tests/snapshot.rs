use gaap_attestation::{
    json,
    snapshot::{SnapshotEntry, SnapshotManifest, content_digest},
};
use std::collections::BTreeMap;
fn entries() -> BTreeMap<String, SnapshotEntry> {
    BTreeMap::from([
        (".".into(), SnapshotEntry::Directory { mode: 0o755 }),
        (
            "source.py".into(),
            SnapshotEntry::File {
                mode: 0o644,
                digest: content_digest(b"public source\n"),
            },
        ),
    ])
}
#[test]
fn contents_modes_and_empty_directories_bind_the_subject() {
    let original = SnapshotManifest::from_entries(entries())
        .unwrap()
        .digest()
        .unwrap();
    let mut changed = entries();
    changed.insert("empty".into(), SnapshotEntry::Directory { mode: 0o755 });
    assert_ne!(
        original,
        SnapshotManifest::from_entries(changed)
            .unwrap()
            .digest()
            .unwrap()
    );
    let mut changed = entries();
    changed.insert(
        "source.py".into(),
        SnapshotEntry::File {
            mode: 0o744,
            digest: content_digest(b"public source\n"),
        },
    );
    assert_ne!(
        original,
        SnapshotManifest::from_entries(changed)
            .unwrap()
            .digest()
            .unwrap()
    );
    let mut changed = entries();
    changed.insert(
        "source.py".into(),
        SnapshotEntry::File {
            mode: 0o644,
            digest: content_digest(b"changed source\n"),
        },
    );
    assert_ne!(
        original,
        SnapshotManifest::from_entries(changed)
            .unwrap()
            .digest()
            .unwrap()
    );
}
#[test]
fn escaping_ambiguous_and_parentless_paths_are_rejected() {
    for path in [
        "../outside",
        "/absolute",
        "a//b",
        "a/./b",
        "a\\b",
        "a/b",
        "a/",
    ] {
        let mut changed = entries();
        changed.insert(path.into(), SnapshotEntry::Directory { mode: 0o755 });
        assert!(SnapshotManifest::from_entries(changed).is_err(), "{path}");
    }
}
#[test]
fn duplicate_keys_invalid_modes_unknown_fields_and_digest_tampering_fail() {
    for bytes in [br#"{".":{"kind":"directory","mode":493},".":{"kind":"directory","mode":0}}"#.as_slice(),br#"{".":{"kind":"directory","mode":9999}}"#.as_slice(),br#"{".":{"kind":"directory","mode":493,"inode":7}}"#.as_slice(),br#"{".":{"kind":"directory","mode":493},"file":{"kind":"file","mode":420,"digest":"bad"}}"#.as_slice()] {
        assert!(SnapshotManifest::parse(bytes).is_err());
    }
}
#[test]
fn canonical_roundtrip_preserves_a_portable_manifest() {
    let first = SnapshotManifest::from_entries(entries()).unwrap();
    let second = SnapshotManifest::parse(&first.canonical_bytes().unwrap()).unwrap();
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
}
#[test]
fn canonical_json_rejects_invalid_interoperability_inputs() {
    for input in [
        "NaN",
        "Infinity",
        "{\"x\":1,\"x\":2}",
        "9007199254740993",
        "\"\\ud800\"",
        "{} trailing",
    ] {
        assert!(json::canonical_bytes(input.as_bytes()).is_err(), "{input}");
    }
    assert_eq!(
        json::canonical_bytes(br#"{"second":2,"first":1}"#).unwrap(),
        br#"{"first":1,"second":2}"#
    );
}
