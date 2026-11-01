// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SUCCESS STATUS of this service's tag mutations,
//! `TagResource` and `UntagResource`, driven over HTTP through the same
//! multi-service router the emulator boots in production.
//!
//! The rule under test: a successful tag mutation answers 204 No Content with a
//! body of zero bytes, because its output structure has no members and so
//! carries nothing for a caller to parse — while an operation whose output DOES
//! carry members, or whose `@http` code the model sets explicitly, keeps that
//! status. A client or proxy that branches on the success status (204 meaning
//! "nothing to parse") depends on the difference.
//!
//! What the two operations DO with the tag set — the merge, the removal and the
//! bound they share — is covered by `tests/all/resource_tags.rs` and
//! `tests/all/untag_resource.rs`; this suite varies the request along what the
//! status rule generalises over (how many keys the mutation carries, whether
//! they are already stored, their sizes, and how many mutations are chained) and
//! asserts the status and the empty body for every row.
//!
//! Shape source: `smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`
//! — both structures have no members.

use serde_json::{json, Map, Value};

use crate::harness::{encode_path_label, Emulator, TAG_MUTATION_SUCCESS_STATUS};

/// One row of the status table: the tag map `TagResource` writes and the keys
/// `UntagResource` then removes.
struct MutationCase {
    case: &'static str,
    written: Vec<(String, String)>,
    removed: Vec<String>,
}

/// The table. Rows differ in the number of keys the mutation carries (none, one,
/// many), whether a removed key is stored or absent, key and value size (both
/// modelled `TagKey` 1..128 / `TagValue` 0..256 maxima, and the empty value the
/// minimum admits), and the alphabet.
fn mutation_cases() -> Vec<MutationCase> {
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    vec![
        MutationCase {
            // `UntagResource`'s `tagKeys` is `@required` and `@httpQuery`, which
            // the REST route table treats as part of route matching, so a
            // removal always names at least one key; the WRITE is the empty map
            // here, which `TagMap`'s `@length` minimum of 0 admits.
            case: "an empty tag map, and one key that was never stored removed",
            written: Vec::new(),
            removed: vec!["never-stored".to_string()],
        },
        MutationCase {
            case: "one key written and removed",
            written: vec![("env".to_string(), "prod".to_string())],
            removed: vec!["env".to_string()],
        },
        MutationCase {
            case: "several keys written, one removed",
            written: vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "platform".to_string()),
                ("tier".to_string(), "1".to_string()),
            ],
            removed: vec!["team".to_string()],
        },
        MutationCase {
            case: "keys that were never stored are removed",
            written: vec![("env".to_string(), "prod".to_string())],
            removed: vec!["absent-a".to_string(), "absent-b".to_string()],
        },
        MutationCase {
            case: "the longest modelled key and value",
            written: vec![(long_key.clone(), long_value)],
            removed: vec![long_key],
        },
        MutationCase {
            case: "an empty value, and punctuation and non-ASCII in the key",
            written: vec![
                ("team:platform/eu-west".to_string(), String::new()),
                ("étiquette".to_string(), "valeur".to_string()),
            ],
            removed: vec!["étiquette".to_string()],
        },
        MutationCase {
            case: "many keys at once",
            written: (0..50)
                .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
                .collect(),
            removed: (0..50).map(|index| format!("key-{index:03}")).collect(),
        },
    ]
}

impl Emulator {
    /// Create a configuration and return its arn, failing the test on anything
    /// other than the modelled 201.
    async fn create_configuration_for_status(&self, name: &str) -> String {
        let body = json!({ "name": name, "description": "a configuration under test" });
        let (status, response) = self
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(
            status, 201,
            "create answers the status its `@http` code states explicitly, got {status} {response:?}"
        );
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create must answer an arn, got {response:?}"))
            .to_string()
    }

    /// `TagResource`, as status and RAW body.
    async fn tag_raw(&self, arn: &str, tags: &[(String, String)]) -> (u16, bytes::Bytes) {
        let mut map = Map::new();
        for (key, value) in tags {
            map.insert(key.clone(), Value::String(value.clone()));
        }
        let uri = format!("/tags/{}", encode_path_label(arn));
        self.send_raw("POST", &uri, Some(json!({ "tags": Value::Object(map) })))
            .await
    }

    /// `UntagResource`, as status and RAW body. `tagKeys` is an `@httpQuery`
    /// list, so each key is one repetition of the parameter.
    async fn untag_raw(&self, arn: &str, keys: &[String]) -> (u16, bytes::Bytes) {
        let query: Vec<String> = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_path_label(key)))
            .collect();
        let uri = format!("/tags/{}?{}", encode_path_label(arn), query.join("&"));
        self.send_raw("DELETE", &uri, None).await
    }
}

/// RULE: a successful tag mutation answers 204 No Content and no body bytes.
#[tokio::test]
async fn a_successful_tag_mutation_answers_no_content() {
    for (index, case) in mutation_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let arn = emulator
            .create_configuration_for_status(&format!("status-case-{index}"))
            .await;

        // The write, then the same write again: the status does not depend on
        // whether the mutation changed anything.
        for attempt in 0..2 {
            let (status, body) = emulator.tag_raw(&arn, &case.written).await;
            assert_eq!(
                status,
                TAG_MUTATION_SUCCESS_STATUS,
                "case {:?} TagResource attempt {attempt}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {status} {:?}",
                case.case,
                String::from_utf8_lossy(&body)
            );
            assert!(
                body.is_empty(),
                "case {:?} TagResource attempt {attempt}: expected no body bytes, got {:?}",
                case.case,
                String::from_utf8_lossy(&body)
            );
        }
        for attempt in 0..2 {
            let (status, body) = emulator.untag_raw(&arn, &case.removed).await;
            assert_eq!(
                status,
                TAG_MUTATION_SUCCESS_STATUS,
                "case {:?} UntagResource attempt {attempt}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {status} {:?}",
                case.case,
                String::from_utf8_lossy(&body)
            );
            assert!(
                body.is_empty(),
                "case {:?} UntagResource attempt {attempt}: expected no body bytes, got {:?}",
                case.case,
                String::from_utf8_lossy(&body)
            );
        }

        // The mutation still happened: the reader reports what is left, with a
        // status and a body of its own.
        let uri = format!("/tags/{}", encode_path_label(&arn));
        let (status, reported) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource carries members, so it answers 200, got {status} {reported:?}",
            case.case
        );
        let remaining = reported["tags"].as_object().unwrap_or_else(|| {
            panic!(
                "case {:?}: ListTagsForResource must answer a tags map, got {reported:?}",
                case.case
            )
        });
        for key in &case.removed {
            assert!(
                !remaining.contains_key(key),
                "case {:?}: {key:?} was removed and must be gone, got {reported:?}",
                case.case
            );
        }
        for (key, value) in &case.written {
            if case.removed.contains(key) {
                continue;
            }
            assert_eq!(
                remaining.get(key).and_then(Value::as_str),
                Some(value.as_str()),
                "case {:?}: {key:?} was written and must still report {value:?}, got {reported:?}",
                case.case
            );
        }
    }
}

/// RULE: an answer that carries no content states the length that body has —
/// zero — so a client framing by the stated length is not left waiting for a
/// body. The whole framing rule, and what of it survives onto a socket, is
/// `tests/all/no_content_framing.rs`.
#[tokio::test]
async fn a_no_content_answer_states_its_empty_body_length() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_for_status("status-framing")
        .await;

    // Rows: both tag mutations, each in the shape its own binding takes.
    let tag_uri = format!("/tags/{}", encode_path_label(&arn));
    let untag_uri = format!("/tags/{}?tagKeys=env", encode_path_label(&arn));
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "TagResource",
            "POST",
            tag_uri.as_str(),
            Some(json!({ "tags": { "env": "prod" } })),
        ),
        ("UntagResource", "DELETE", untag_uri.as_str(), None),
    ];

    for (case, method, uri, body) in cases {
        let response = emulator.send_http(method, uri, body).await;
        assert_eq!(
            response.status().as_u16(),
            TAG_MUTATION_SUCCESS_STATUS,
            "case {case}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {}",
            response.status()
        );
        assert!(
            response.body().is_empty(),
            "case {case}: expected no body bytes, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        let framing = response
            .headers()
            .get("content-length")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
        assert_eq!(
            framing.as_deref(),
            Some("0"),
            "case {case}: a no-content answer states the zero length of its body, got {framing:?}"
        );
    }
}

/// RULE: the no-content status belongs to the empty-bodied mutations only — an
/// answer that carries members keeps the status the model states for it, so the
/// change is not a blanket 204.
#[tokio::test]
async fn answers_that_carry_members_keep_their_own_status() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_for_status("status-contrast")
        .await;
    let (status, _) = emulator.tag_raw(&arn, &[("env".to_string(), "prod".to_string())]).await;
    assert_eq!(
        status, TAG_MUTATION_SUCCESS_STATUS,
        "case TagResource: expected {TAG_MUTATION_SUCCESS_STATUS}, got {status}"
    );

    // Rows differ in the kind of answer: a create whose `@http` code the model
    // sets explicitly (201), and readers whose output carries members (200).
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "ListTagsForResource",
            format!("/tags/{}", encode_path_label(&arn)),
            200,
        ),
        (
            "GetNotificationConfiguration",
            format!("/notification-configurations/{}", encode_path_label(&arn)),
            200,
        ),
        (
            "ListNotificationConfigurations",
            "/notification-configurations".to_string(),
            200,
        ),
        (
            "ListManagedNotificationConfigurations",
            "/managed-notification-configurations".to_string(),
            200,
        ),
    ];

    for (case, uri, expected) in cases {
        let (status, body) = emulator.send_raw("GET", &uri, None).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} {:?}",
            String::from_utf8_lossy(&body)
        );
        assert!(
            !body.is_empty(),
            "case {case}: an answer carrying members must carry a body, got none"
        );
    }
}
