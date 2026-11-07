use rsrazer_daemon::protocol::{blackshark, ble, builders::bytes, kraken};
use serde_json::{json, Value};

#[test]
fn headset_commands_match_the_python_backend() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/protocol.json")).unwrap();
    let cases = &fixture["headsets"];
    for case in cases["ble"].as_array().unwrap() {
        let packet = match case["operation"].as_str().unwrap() {
            "brightness" => {
                let level = case["level"].as_i64().unwrap();
                assert_eq!(json!(ble::level_to_percentage(level)), case["percent"]);
                ble::brightness_command(level).to_vec()
            }
            "percentage" => {
                assert_eq!(
                    json!(ble::percentage_to_level(case["percent"].as_f64().unwrap()).unwrap()),
                    case["level"]
                );
                continue;
            }
            "colour" => {
                let colours: Vec<[u8; 3]> =
                    serde_json::from_value(case["colours"].clone()).unwrap();
                ble::colour_command(&colours).unwrap()
            }
            "read" => ble::read_request(case["opcode"].as_u64().unwrap() as u8).to_vec(),
            "takeover" => ble::takeover_command().to_vec(),
            "off" => ble::off_command().to_vec(),
            other => panic!("unknown BLE fixture {other}"),
        };
        assert_eq!(packet, bytes(&case["packet"]).unwrap(), "{case}");
    }
    for case in cases["blackshark"].as_array().unwrap() {
        let packet = blackshark::request(
            case["sequence"].as_u64().unwrap() as u8,
            case["domain"].as_u64().unwrap() as u8,
            case["command"].as_u64().unwrap() as u8,
            &bytes(&case["payload"]).unwrap(),
        )
        .unwrap();
        assert_eq!(packet.as_slice(), bytes(&case["packet"]).unwrap(), "{case}");
    }
    for case in cases["eq"].as_array().unwrap() {
        let gains: Vec<i64> = serde_json::from_value(case["gains"].clone()).unwrap();
        assert_eq!(
            json!(blackshark::encode_bands(&gains).unwrap()),
            case["encoded"]
        );
        assert_eq!(
            json!(blackshark::decode_bands(&bytes(&case["stored"]).unwrap()).unwrap()),
            case["decoded"]
        );
    }
    for case in cases["kraken"].as_array().unwrap() {
        let pid = case["pid"].as_u64().unwrap() as u16;
        let layout = kraken::layout_for(pid).unwrap();
        let colours = [[11, 22, 33], [44, 55, 66], [77, 88, 99]];
        let operation = case["operation"].as_str().unwrap();
        let packets = match operation {
            "none" => kraken::effect_none(layout),
            "spectrum" => kraken::effect_spectrum(layout),
            "static" => {
                kraken::effect_static(layout, colours[0], Some(127), kraken::has_colour(pid))
            }
            "custom" => kraken::effect_custom(layout, colours[0], Some(43)),
            value if value.starts_with("breathing_") => {
                kraken::effect_breathing(layout, &colours[..value[10..].parse::<usize>().unwrap()])
            }
            _ => panic!("unknown Kraken fixture {operation}"),
        }
        .unwrap();
        let packets: Vec<Vec<u8>> = packets.iter().map(|p| p.to_vec()).collect();
        assert_eq!(json!(packets), case["packets"], "{pid:04x} {operation}");
    }
}

fn shark_reply(
    sequence: u8,
    domain: u8,
    command: u8,
    status: u8,
    ack: u8,
    data: &[u8],
    include_id: bool,
) -> [u8; 64] {
    let mut report = blackshark::request(sequence, domain, command, data).unwrap();
    report[1] = status;
    report[8] = 128;
    report[11] = ack;
    report[62] = report[usize::from(!include_id)..62]
        .iter()
        .fold(0, |sum, byte| sum ^ byte);
    report
}

#[test]
fn blackshark_matches_sequence_domain_command_and_checksum() {
    for include_id in [false, true] {
        let response = shark_reply(4, 128, 0x21, 2, 1, &[87], include_id);
        assert!(blackshark::answers(&response, 4, 128, 0x21, false));
        assert!(blackshark::answers(&response, 4, 0, 0x21, true));
        assert!(!blackshark::answers(&response, 4, 0, 0x21, false));
        assert!(!blackshark::answers(&response, 5, 128, 0x21, false));
        assert!(!blackshark::answers(&response, 4, 128, 0x2a, false));
        assert_eq!(blackshark::result(&response, 0x21).unwrap(), &[87]);
        let mut corrupted = response;
        corrupted[20] ^= 8;
        assert!(!blackshark::is_reply(&corrupted));
        for length in 0..64 {
            assert!(!blackshark::is_reply(&response[..length]));
        }
    }
}

#[test]
fn blackshark_acknowledgment_does_not_imply_a_successful_write() {
    for status in [0, 1, 3, 4, 5, 255] {
        assert!(
            blackshark::result(&shark_reply(0, 128, 0x99, status, 1, &[0], false), 0x99).is_err()
        );
    }
    for (ack, payload) in [(0, vec![0]), (1, vec![]), (1, vec![1])] {
        assert!(
            blackshark::result(&shark_reply(0, 128, 0x99, 2, ack, &payload, false), 0x99).is_err()
        );
    }
    assert!(blackshark::result(&shark_reply(0, 128, 0x99, 2, 1, &[0], false), 0x99).is_ok());
    assert!(blackshark::result(&[], 0x21).is_err());
    assert!(blackshark::request(0, 0, 0, &[0; 49]).is_err());
    assert!(blackshark::encode_bands(&[-10; 10]).is_err());
    assert!(blackshark::encode_bands(&[0; 9]).is_err());
    assert!(blackshark::decode_bands(&[0; 11]).is_err());
}

#[test]
fn ble_framing_and_manufacturer_advertisement_are_bounded() {
    let data = [5, 0x62, 0, 0x60, 0x34, 0x57, 0xcd, 0x5e, 0x44, 0];
    assert_eq!(ble::product_id(&data), Some(0x0562));
    assert_eq!(ble::classic_address(&data), Some(0x445ecd573460));
    assert_eq!(ble::product_id(&data[..1]), None);
    assert_eq!(ble::classic_address(&data[..8]), None);
    assert_eq!(ble::read_opcode(0xc1), 0x41);
    assert_eq!(ble::write_opcode(0x27), 0xa7);
    for kind in [0, 1, 2] {
        let message = ble::message(0x21, &[87], kind).unwrap();
        assert_eq!(
            ble::parse_response(&message).unwrap(),
            (0x21, kind, [87].as_slice())
        );
        for length in 0..message.len() {
            assert!(ble::parse_response(&message[..length]).is_err());
        }
    }
    assert!(ble::message(0x21, &[0; 256], 0).is_err());
    assert!(ble::colour_command(&[]).is_err());
    assert!(ble::percentage_to_level(f64::NAN).is_err());
    assert!(kraken::read_ram(0x1189, 33).is_err());
    assert!(kraken::effect_breathing(kraken::RAINIE, &[[0; 3]; 2]).is_err());
}
