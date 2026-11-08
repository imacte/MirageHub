use rsrazer_daemon::{database, protocol::builders};
use serde_json::Value;

#[test]
fn every_packet_matches_the_python_backend() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/protocol.json")).unwrap();
    let cases = fixture["builders"].as_array().unwrap();
    assert!(cases.len() > 700, "the full builder corpus must be present");
    for (index, case) in cases.iter().enumerate() {
        let name = case["builder"].as_str().unwrap();
        let args = case["args"].as_array().unwrap();
        let packet = builders::build(name, args)
            .unwrap_or_else(|e| panic!("case {index}: {name} {args:?}: {e}"));
        let expected = builders::bytes(&case["packet"]).unwrap();
        assert_eq!(
            packet.pack().as_slice(),
            expected,
            "case {index}: {name} {args:?}"
        );
    }
}

#[test]
fn every_device_preserves_its_public_metadata() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/protocol.json")).unwrap();
    let expected = fixture["devices"].as_array().unwrap();
    assert_eq!(database::devices().len(), expected.len());
    for (device, expected) in database::devices().iter().zip(expected) {
        let mut expected = expected.clone();
        if matches!(device.pid, 0x00a7 | 0x00a8) {
            // #2962 adds these methods after the original Python corpus was
            // captured. Keep that baseline intact and assert the exact delta.
            let methods = expected["methods"].as_array_mut().unwrap();
            methods.extend(
                [
                    "add_onboard_macro",
                    "delete_onboard_macro",
                    "get_button_action",
                    "get_onboard_buttons",
                    "get_onboard_macro_storage",
                    "get_onboard_macros",
                    "get_side_plate",
                    "set_button_action",
                ]
                .map(|name| Value::String(name.into())),
            );
            methods.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
        assert_eq!(device.to_json(), expected, "{}", device.key());
    }
}

#[test]
fn malformed_builder_input_returns_errors() {
    use serde_json::json;
    assert!(builders::build("not_a_builder", &[]).is_err());
    assert!(builders::build("get_razer_report", &[]).is_err());
    assert!(builders::build(
        "razer_chroma_standard_matrix_effect_static",
        &[json!("red")]
    )
    .is_err());
    assert!(builders::build(
        "razer_chroma_misc_set_dpi_stages",
        &[json!(1), json!(5), json!(0), json!([])]
    )
    .is_err());
    assert!(builders::build(
        "razer_chroma_standard_matrix_set_custom_frame",
        &[json!(0), json!(9), json!(1), json!([])]
    )
    .is_err());
}
