// SPDX-License-Identifier: CC0-1.0

//! What this service answers when a successful call has nothing to report.
//!
//! RULE under test: a successful call whose answer conveys nothing but its status — the
//! operation's output carries no members — is answered `204 No Content` with NO body,
//! stating the length that answer has (`content-length: 0`), while a call whose answer
//! carries content keeps its `200` and its document. The two tag MUTATIONS are the
//! operations whose status the model leaves defaulted (`smithy_query(query="operation",
//! operation="TagResource,UntagResource")` reports `code: 200, codeDefaulted: true`), and
//! the recorded AWS answers are what state it; the two removals the same model pins
//! explicitly (`DeleteRoom` and `DeleteLoggingConfiguration`, `@http code 204`) are the
//! same rule written down.
//!
//! The framing is what this service answers and what the in-process front door delivers
//! (the router's response is served unchanged). On an answer served over HTTP/1.1 the
//! header does not survive this status: the HTTP writer the vendored transport uses removes
//! `content-length` from a 204, which is below anything this crate decides.
//!
//! Every row is a different input along what the rule generalises over (the operation, its
//! HTTP method, the resource kind it names, the size of the tag set written or removed, and
//! whether the keys it names are ones the resource carries), never a rename of one case.
//!
//! The router, the signed request and the `/tags/{resourceArn}` label encoder are the ones
//! `validation_rules_and_tagging.rs` already owns; only the framing assertions are here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::state::NO_CONTENT_SUCCESS_STATUS;

use crate::validation_rules_and_tagging::{encode_label, Answer, Emulator};

/// The status an answer that carries a document is served with: the Smithy default, which
/// for a reporting operation of this service is also what AWS answers.
const WITH_CONTENT_STATUS: u16 = 200;

/// The length an answer with no body states.
const NO_CONTENT_LENGTH: &str = "0";

/// The three facts the no-content rule is about: the status, whether the answer carries a
/// body, and the length it states.
fn framing(answer: &Answer) -> (u16, bool, Option<&str>) {
    (
        answer.status,
        answer.body.is_empty(),
        answer.content_length.as_deref(),
    )
}

/// The JSON document an answer carries.
fn document(answer: &Answer) -> Value {
    serde_json::from_slice(&answer.body)
        .unwrap_or_else(|e| panic!("expected a JSON document, got {:?}: {e}", answer.text()))
}

/// `POST /tags/{resourceArn}` with a tag map, read for its framing.
async fn tag(emulator: &Emulator, resource_arn: &str, tags: &Value) -> Answer {
    emulator
        .call_raw(
            "POST",
            &format!("/tags/{}", encode_label(resource_arn)),
            &json!({ "tags": tags }).to_string(),
        )
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `tagKeys` is an `@httpQuery` list — read for
/// its framing.
async fn untag(emulator: &Emulator, resource_arn: &str, tag_keys: &[&str]) -> Answer {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect();
    emulator
        .call_raw(
            "DELETE",
            &format!("/tags/{}?{}", encode_label(resource_arn), query.join("&")),
            "",
        )
        .await
}

/// The tag set a resource carries, as `ListTagsForResource` reports it.
async fn stored_tags(emulator: &Emulator, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, listed) = emulator
        .call("GET", &format!("/tags/{}", encode_label(resource_arn)), "")
        .await;
    assert_eq!(
        status, WITH_CONTENT_STATUS,
        "ListTagsForResource must answer {WITH_CONTENT_STATUS}, got {status} {listed}"
    );
    listed
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A logging configuration, created with the simplest destination the modelled
/// `DestinationConfiguration` union admits, answered as the document the create renders.
async fn create_logging_configuration(emulator: &Emulator) -> Value {
    let (status, created) = emulator
        .post(
            "CreateLoggingConfiguration",
            &json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs" } } }),
        )
        .await;
    assert_eq!(
        status, WITH_CONTENT_STATUS,
        "CreateLoggingConfiguration must answer {WITH_CONTENT_STATUS}, got {status} {created}"
    );
    created
}

/// The ARN a create reports, which is how every row names its resource.
fn arn_of(created: &Value) -> String {
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "the fixture resource must carry an ARN, got {created}"
    );
    arn
}

/// Which resource a row's call names.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// A room, created with three tags so a removal has keys to name.
    TaggedRoom,
    /// A logging configuration, named by the ARN its create answers.
    LoggingConfiguration,
}

/// The call a row makes against the resource it is given.
#[derive(Debug, Clone)]
enum Call {
    /// `TagResource` with this tag map.
    Tag(Value),
    /// `UntagResource` naming these keys.
    Untag(Vec<&'static str>),
    /// One of the `POST /<Operation>` bindings, naming the resource in `identifier`.
    ByIdentifier(&'static str),
}

/// The three tags every `TaggedRoom` row starts from, so a removal names keys the resource
/// carries and a write can replace one.
fn starting_tags() -> Value {
    json!({ "env": "sim2real", "keep": "me", "owner": "team" })
}

/// RULE: an answer that conveys nothing but its status is a 204 with no body, stating the
/// length it has — for every operation of this service whose output carries no members,
/// whichever way the model states that status.
#[tokio::test]
async fn an_answer_that_conveys_only_its_status_is_an_empty_204_that_states_its_length() {
    let emulator = Emulator::new();

    // Rows differ in the operation, its HTTP method, the resource kind it names, the size
    // of the tag set it writes or removes, and whether the keys it names are carried.
    let rows: Vec<(&str, Subject, Call)> = vec![
        (
            "TagResource, one new key",
            Subject::TaggedRoom,
            Call::Tag(json!({ "tier": "gold" })),
        ),
        (
            "TagResource, three new keys",
            Subject::TaggedRoom,
            Call::Tag(json!({ "a": "1", "b": "2", "c": "3" })),
        ),
        (
            "TagResource, a key the room already carries",
            Subject::TaggedRoom,
            Call::Tag(json!({ "env": "replaced" })),
        ),
        (
            "TagResource, no keys at all",
            Subject::TaggedRoom,
            Call::Tag(json!({})),
        ),
        (
            "UntagResource, one of the room's keys",
            Subject::TaggedRoom,
            Call::Untag(vec!["env"]),
        ),
        (
            "UntagResource, every key the room carries",
            Subject::TaggedRoom,
            Call::Untag(vec!["env", "keep", "owner"]),
        ),
        (
            "UntagResource, a key the room does not carry",
            Subject::TaggedRoom,
            Call::Untag(vec!["absent"]),
        ),
        (
            "DeleteRoom, whose 204 the model pins explicitly",
            Subject::TaggedRoom,
            Call::ByIdentifier("DeleteRoom"),
        ),
        (
            "DeleteLoggingConfiguration, whose 204 the model pins explicitly",
            Subject::LoggingConfiguration,
            Call::ByIdentifier("DeleteLoggingConfiguration"),
        ),
    ];

    for (case, subject, call) in rows {
        let created = match subject {
            Subject::TaggedRoom => {
                emulator
                    .create_room(&json!({ "name": "no-content", "tags": starting_tags() }))
                    .await
            }
            Subject::LoggingConfiguration => create_logging_configuration(&emulator).await,
        };
        let arn = arn_of(&created);

        let answer = match &call {
            Call::Tag(tags) => tag(&emulator, &arn, tags).await,
            Call::Untag(keys) => untag(&emulator, &arn, keys).await,
            Call::ByIdentifier(operation) => {
                emulator
                    .call_raw(
                        "POST",
                        &format!("/{operation}"),
                        &json!({ "identifier": arn }).to_string(),
                    )
                    .await
            }
        };

        assert_eq!(
            framing(&answer),
            (NO_CONTENT_SUCCESS_STATUS, true, Some(NO_CONTENT_LENGTH)),
            "case {case:?}: expected HTTP {NO_CONTENT_SUCCESS_STATUS} with no body stating \
             content-length {NO_CONTENT_LENGTH}, got {} body={:?} content-length={:?}",
            answer.status,
            answer.text(),
            answer.content_length
        );
    }
}

/// RULE, the other half: a call whose answer HAS content keeps the 200 the model defaults
/// to and carries the document that content lives in.
#[tokio::test]
async fn an_answer_that_carries_content_keeps_its_200_and_its_document() {
    let emulator = Emulator::new();
    let room = emulator
        .create_room(&json!({ "name": "with-content", "tags": starting_tags() }))
        .await;
    let arn = arn_of(&room);
    let configuration = create_logging_configuration(&emulator).await;
    let configuration_arn = arn_of(&configuration);

    // Rows differ in the operation, its HTTP method and the member its document reports:
    // the tagging REPORT on the same `/tags/{resourceArn}` endpoint the two mutations use,
    // a resource read, a listing, and an update.
    let rows: Vec<(&str, &str, String, String, &str)> = vec![
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", encode_label(&arn)),
            String::new(),
            "tags",
        ),
        (
            "GetRoom",
            "POST",
            "/GetRoom".to_string(),
            json!({ "identifier": arn }).to_string(),
            "arn",
        ),
        (
            "ListRooms",
            "POST",
            "/ListRooms".to_string(),
            json!({}).to_string(),
            "rooms",
        ),
        (
            "UpdateRoom",
            "POST",
            "/UpdateRoom".to_string(),
            json!({ "identifier": arn, "name": "renamed" }).to_string(),
            "arn",
        ),
        (
            "GetLoggingConfiguration",
            "POST",
            "/GetLoggingConfiguration".to_string(),
            json!({ "identifier": configuration_arn }).to_string(),
            "destinationConfiguration",
        ),
    ];

    for (case, method, uri, body, member) in rows {
        let answer = emulator.call_raw(method, &uri, &body).await;
        assert_eq!(
            answer.status,
            WITH_CONTENT_STATUS,
            "case {case:?}: expected HTTP {WITH_CONTENT_STATUS}, got {} {:?}",
            answer.status,
            answer.text()
        );
        assert!(
            document(&answer).get(member).is_some(),
            "case {case:?}: expected the document to report {member:?}, got {:?}",
            answer.text()
        );
    }
}

/// One row of the landed-write table: the case, the tags written, the keys removed and the
/// tag set the resource is left holding.
type LandedRow = (&'static str, Value, Vec<&'static str>, BTreeMap<String, String>);

/// RULE: the 204 reports a write that LANDED — the status says there is nothing to read
/// back, not that nothing happened, so the tag set the reporting operation answers is the
/// one the mutations left behind.
#[tokio::test]
async fn a_tag_mutation_that_answers_204_has_changed_the_stored_tag_set() {
    let emulator = Emulator::new();

    // Rows differ in how many keys the write adds or replaces and how many the removal
    // names, so the empty answer is checked against a different stored set each time.
    let rows: Vec<LandedRow> = vec![
        (
            "add one key, remove one",
            json!({ "tier": "gold" }),
            vec!["env"],
            BTreeMap::from([
                ("keep".to_string(), "me".to_string()),
                ("owner".to_string(), "team".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ]),
        ),
        (
            "replace a carried key, remove two",
            json!({ "env": "replaced" }),
            vec!["keep", "owner"],
            BTreeMap::from([("env".to_string(), "replaced".to_string())]),
        ),
        (
            "add two keys, remove none that are carried",
            json!({ "a": "1", "b": "2" }),
            vec!["absent"],
            BTreeMap::from([
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("env".to_string(), "sim2real".to_string()),
                ("keep".to_string(), "me".to_string()),
                ("owner".to_string(), "team".to_string()),
            ]),
        ),
        (
            "write nothing, remove every key",
            json!({}),
            vec!["env", "keep", "owner"],
            BTreeMap::new(),
        ),
    ];

    for (case, tags, removed, expected) in rows {
        let room = emulator
            .create_room(&json!({ "name": "landed", "tags": starting_tags() }))
            .await;
        let arn = arn_of(&room);

        let written = tag(&emulator, &arn, &tags).await;
        assert_eq!(
            framing(&written),
            (NO_CONTENT_SUCCESS_STATUS, true, Some(NO_CONTENT_LENGTH)),
            "case {case:?}: TagResource must answer an empty {NO_CONTENT_SUCCESS_STATUS}, \
             got {} body={:?} content-length={:?}",
            written.status,
            written.text(),
            written.content_length
        );

        let cleared = untag(&emulator, &arn, &removed).await;
        assert_eq!(
            framing(&cleared),
            (NO_CONTENT_SUCCESS_STATUS, true, Some(NO_CONTENT_LENGTH)),
            "case {case:?}: UntagResource must answer an empty {NO_CONTENT_SUCCESS_STATUS}, \
             got {} body={:?} content-length={:?}",
            cleared.status,
            cleared.text(),
            cleared.content_length
        );

        let actual = stored_tags(&emulator, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tag set {expected:?}, got {actual:?}"
        );
    }
}
