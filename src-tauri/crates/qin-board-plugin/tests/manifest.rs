//! The embedded manifest must pass the host's own parser and validator.
use se_plugin_protocol::manifest::parse_manifest;

#[test]
fn embedded_manifest_parses_and_validates() {
    let manifest = parse_manifest(qin_board_plugin::MANIFEST).expect("manifest parses");
    let value: serde_json::Value = serde_json::from_slice(qin_board_plugin::MANIFEST).unwrap();
    assert_eq!(value["id"], "org.qincode.board");
    let commands: Vec<_> = value["contributes"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(commands, ["endpoint", "restart-service", "open-board"]);
    assert_eq!(value["contributes"]["processes"][0]["restart"], "always");
    drop(manifest);
}
