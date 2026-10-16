use gaap_attestation::json;
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn node_and_rust_agree_on_unicode_numbers_objects_and_arrays() {
    // Non-English protocol fixtures exercise UTF-16 property sorting and UTF-8 strings.
    let vectors = [
        r#"{"numbers":[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001,-0.0,1e21],"string":"\u20ac$\u000f\nA'B\"\\\"/","literals":[null,true,false]}"#,
        r#"{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter","1":"One","\ud83d\ude00":"Emoji","\u0080":"Control","\u00f6":"Latin"}"#,
        r#"{"z":[{"b":2,"a":1},[3,2,1]],"a":{"\ud83d\ude00":1,"\ue000":2}}"#,
    ];
    for input in vectors {
        let expected = json::canonical_bytes(input.as_bytes()).unwrap();
        let mut child = Command::new("node").args(["-e",r#"
let input=''; process.stdin.setEncoding('utf8'); process.stdin.on('data',part=>input+=part);
function jcs(value) {
 if (Array.isArray(value)) return '['+value.map(jcs).join(',')+']';
 if (value && typeof value === 'object') return '{'+Object.keys(value).sort().map(key=>JSON.stringify(key)+':'+jcs(value[key])).join(',')+'}';
 return JSON.stringify(value);
}
process.stdin.on('end',()=>process.stdout.write(jcs(JSON.parse(input))));
"#]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(expected, output.stdout);
    }
}
#[test]
fn invalid_canonicalization_inputs_cant_receive_an_identity() {
    for input in [
        r#"{"a":{"x":1,"x":2}}"#,
        r#"["\ud800"]"#,
        "[NaN]",
        "[Infinity]",
        "[1e400]",
        "[9007199254740992]",
        "{} true",
    ] {
        assert!(json::canonical_bytes(input.as_bytes()).is_err(), "{input}");
    }
}
