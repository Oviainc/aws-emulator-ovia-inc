// SPDX-License-Identifier: CC0-1.0

//! The FORM of the OpenSSH private-key container a caller supplies host key material in.
//!
//! RULE: the container's armour is not part of the key. What identifies an OpenSSH
//! private key is the Base64 payload between the `-----BEGIN OPENSSH PRIVATE KEY-----`
//! and `-----END OPENSSH PRIVATE KEY-----` boundary lines, so the column that payload is
//! wrapped at, the line ending it uses and any surrounding blank space all name the SAME
//! key: every member that takes private key material (`ImportHostKey`'s `HostKeyBody`,
//! `CreateServer`'s and `UpdateServer`'s `HostKey`) reads them identically and reports
//! that one key's own fingerprint and algorithm.
//!
//! The rows differ along what the rule generalises over — the wrap column (the 70 columns
//! OpenSSH writes, the 64 another tool writes, 76, and the whole payload on one line), the
//! line ending (LF, CRLF) and the surrounding blank space — over key material this suite
//! generates with the OpenSSH key-format crate, so every expectation comes from the key
//! rather than from the handler under test.
//!
//! Re-wrapping is not REPAIR: a payload that is not a key is still refused, whichever
//! armour it arrives in.

use serde_json::json;

use crate::harness::{generated_host_key, host_key_algorithm, member_of, Emulator};

/// The boundary lines of the OpenSSH private-key container.
const BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const END: &str = "-----END OPENSSH PRIVATE KEY-----";

/// One armour form: how a tool that wrote this key would have laid its container out.
struct Armour {
    case: &'static str,
    /// The Base64 column the payload is wrapped at.
    width: usize,
    /// The line ending between the lines.
    line_ending: &'static str,
    /// Text put before the container and after it (blank space a caller may paste in).
    surrounding: &'static str,
    /// Whether the container's last line ends with a line ending at all.
    trailing_newline: bool,
}

/// The armour forms the rule covers.
const ARMOURS: &[Armour] = &[
    Armour {
        case: "the 70 columns OpenSSH itself writes",
        width: 70,
        line_ending: "\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "64 columns, as other key libraries write",
        width: 64,
        line_ending: "\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "76 columns",
        width: 76,
        line_ending: "\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "the whole payload on one line",
        width: usize::MAX,
        line_ending: "\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "CRLF line endings",
        width: 70,
        line_ending: "\r\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "64 columns with CRLF line endings",
        width: 64,
        line_ending: "\r\n",
        surrounding: "",
        trailing_newline: true,
    },
    Armour {
        case: "no trailing newline",
        width: 64,
        line_ending: "\n",
        surrounding: "",
        trailing_newline: false,
    },
    Armour {
        case: "surrounded by blank space",
        width: 64,
        line_ending: "\n",
        surrounding: "\n  ",
        trailing_newline: true,
    },
];

/// `material`'s payload re-armoured as `armour` lays a container out.
fn rearmour(armour: &Armour, payload: &str) -> String {
    let mut out = String::new();
    out.push_str(armour.surrounding);
    out.push_str(BEGIN);
    out.push_str(armour.line_ending);
    let characters: Vec<char> = payload.chars().collect();
    let width = armour.width.min(characters.len().max(1));
    for line in characters.chunks(width) {
        out.extend(line.iter());
        out.push_str(armour.line_ending);
    }
    out.push_str(END);
    if armour.trailing_newline {
        out.push_str(armour.line_ending);
    }
    out.push_str(armour.surrounding);
    out
}

/// The Base64 payload of an OpenSSH private-key container, with its armour stripped.
fn payload_of(material: &str) -> String {
    material
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect()
}

/// RULE (see the module docs): every armour form of one key imports as that key, with the
/// key's own fingerprint and algorithm reported back.
#[tokio::test]
async fn host_key_material_is_read_whatever_armour_it_arrives_in() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    for armour in ARMOURS {
        let case = armour.case;
        // Fresh material per row: the same material twice on one server is one key, which
        // the import refuses — so each row carries its own key, and the fingerprint it
        // expects is that key's.
        let (written, fingerprint) = generated_host_key();
        let body = rearmour(armour, &payload_of(&written));

        let imported = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": body}),
            )
            .await;
        assert_eq!(
            imported.status, 200,
            "case {case:?}: expected the import of an OpenSSH private key to be accepted, \
             got {} with body {}",
            imported.status, imported.body
        );
        let host_key_id = imported
            .get("HostKeyId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a HostKeyId, got {}", imported.body))
            .to_string();

        let described = emulator
            .call(
                "DescribeHostKey",
                json!({"ServerId": &server_id, "HostKeyId": &host_key_id}),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeHostKey 200, got {} with body {}",
            described.status, described.body
        );
        let record = described.get("HostKey").clone();
        assert_eq!(
            member_of(&record, "HostKeyFingerprint").as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: expected the imported key's own fingerprint {fingerprint:?}, \
             got {record}"
        );
        let algorithm = host_key_algorithm(&written);
        assert_eq!(
            member_of(&record, "Type").as_str(),
            Some(algorithm.as_str()),
            "case {case:?}: expected the imported key's own algorithm {algorithm:?}, got {record}"
        );
    }
}

/// RULE: the same reading applies to the `HostKey` a server is CREATED with — the member
/// that sets the server's own host key — so the fingerprint `DescribeServer` reports is
/// that key's, whichever armour the material arrived in.
#[tokio::test]
async fn a_servers_supplied_host_key_is_read_whatever_armour_it_arrives_in() {
    let emulator = Emulator::new();

    for armour in ARMOURS {
        let case = armour.case;
        let (written, fingerprint) = generated_host_key();
        let body = rearmour(armour, &payload_of(&written));

        let server_id = emulator.create_server(json!({"HostKey": body})).await;
        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeServer 200, got {} with body {}",
            described.status, described.body
        );
        let server = described.get("Server").clone();
        assert_eq!(
            member_of(&server, "HostKeyFingerprint").as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: expected the server to report the supplied key's own fingerprint \
             {fingerprint:?}, got {server}"
        );
    }
}

/// RULE: re-reading the armour is not repairing the key. A container whose payload is not
/// a private key is refused with `InvalidRequestException` (400) in every armour form, and
/// mints nothing.
#[tokio::test]
async fn a_container_whose_payload_is_not_a_key_is_still_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let (written, _) = generated_host_key();
    let payload = payload_of(&written);
    // The rows are the ways a payload can fail to be a key: truncated, with a character
    // the Base64 alphabet does not carry, and empty.
    let corrupted: Vec<(String, String)> = vec![
        (
            "a truncated payload".to_string(),
            payload[..payload.len() / 2].to_string(),
        ),
        (
            "a payload carrying a character Base64 does not".to_string(),
            format!("{}!!", &payload[..payload.len() - 2]),
        ),
        ("an empty payload".to_string(), String::new()),
    ];

    for (what, broken) in corrupted {
        for armour in ARMOURS {
            let case = format!("{what} in {}", armour.case);
            let answer = emulator
                .call(
                    "ImportHostKey",
                    json!({"ServerId": &server_id, "HostKeyBody": rearmour(armour, &broken)}),
                )
                .await;
            assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
                answer.status,
                answer.body
            );
            assert!(
                member_of(&answer.body, "HostKeyId").is_null(),
                "case {case:?}: expected a refusal to mint no id, got {}",
                answer.body
            );
        }
    }
}
