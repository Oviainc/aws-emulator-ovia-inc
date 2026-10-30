// SPDX-License-Identifier: CC0-1.0

//! The SUCCESS STATUS and the framing of this service's tag mutations, `TagResource`
//! and `UntagResource`, driven over the production `MultiServiceRouter` so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! RULE: a successful tag mutation answers 204 No Content with zero body bytes, stating
//! the length it has — the response structure carries no members, so the
//! service reports "nothing to parse" with the status that says so rather than with the
//! 200 the protocol leaves defaulted. Every other operation keeps its own success
//! status: the create answers the code the model states for it, and the readers, whose
//! output carries members, answer 200.
//!
//! The rule covers the whole class of tag mutations, not one request shape, so the rows
//! below vary along what it generalises over — how many pairs the write carries, whether
//! a removed key is stored or absent, the key and value sizes the modelled `TagKey` and
//! `TagValue` bounds admit, the alphabet, and how often the same call is repeated — and
//! the status is asserted for every row.
//!
//! Shape source:
//! `smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse",model_path="services/service-com-amazonaws-notificationscontacts-notificationscontacts/smithy-model/com-amazonaws-notificationscontacts-notificationscontacts.json")`
//! — both structures have no members, and neither operation states an `@http` code.
//!
//! What the two operations DO to the tag set — the merge, the removal, the bound and the
//! 404 — is owned by `resource_tag_writes.rs`; this suite owns the status alone. The
//! harness is the one `email_contacts.rs` builds.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::email_contacts::{
    activate, call, call_http, create, create_ok, delete, emulator, get, issued_code, label, list,
    send_code, Emulator,
};
use crate::resource_tag_writes::{query_value, TAG_MUTATION_SUCCESS_STATUS};
use crate::resource_tags::{
    create_tagged, list_tags, pair, tag_map, tags_ok, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};

/// One row of the status table: the pairs `TagResource` writes onto a contact that
/// already carries `stored`, and the keys `UntagResource` then names.
struct StatusCase {
    what: &'static str,
    stored: BTreeMap<String, String>,
    written: BTreeMap<String, String>,
    removed: Vec<String>,
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The table. Rows differ in how many pairs the write carries (none, one, many), whether
/// the removal names a stored key or one that was never there, the key and value sizes
/// the modelled bounds admit (`TagKey` 1..128, `TagValue` 0..256), and the alphabet.
fn status_table() -> Vec<StatusCase> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        StatusCase {
            what: "an empty tag map onto an untagged contact, removing a key never stored",
            stored: BTreeMap::new(),
            written: BTreeMap::new(),
            removed: keys(&["never-stored"]),
        },
        StatusCase {
            what: "one pair written and the same key removed",
            stored: BTreeMap::new(),
            written: tag_map([pair("team", "infra")]),
            removed: keys(&["team"]),
        },
        StatusCase {
            what: "several pairs written, one of them removed",
            stored: tag_map([pair("env", "dev")]),
            written: tag_map([
                pair("team", "infra"),
                pair("env", "prod"),
                pair("owner", "on-call"),
            ]),
            removed: keys(&["env"]),
        },
        StatusCase {
            what: "a write that only replaces values, and a removal naming absent keys",
            stored: tag_map([pair("team", "infra")]),
            written: tag_map([pair("team", "platform")]),
            removed: keys(&["absent-a", "absent-b"]),
        },
        StatusCase {
            what: "a key at the TagKey maximum and a value at the TagValue maximum",
            stored: BTreeMap::new(),
            written: tag_map([(long_key.clone(), long_value)]),
            removed: vec![long_key],
        },
        StatusCase {
            what: "an empty value, the TagValue minimum, and a one-character key",
            stored: BTreeMap::new(),
            written: tag_map([pair("retired", ""), pair("k", "short")]),
            removed: keys(&["retired"]),
        },
        StatusCase {
            what: "keys and values carrying punctuation and non-ascii text",
            stored: tag_map([pair("cost:center", "eu-west/1")]),
            written: tag_map([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve value"),
                pair("spaced key", "spaced value"),
            ]),
            removed: keys(&["équipe", "spaced key"]),
        },
        StatusCase {
            what: "fifty pairs written and every one of them removed",
            stored: BTreeMap::new(),
            written: (0..50)
                .map(|n| pair(&format!("key-{n:03}"), &format!("value-{n:03}")))
                .collect(),
            removed: (0..50).map(|n| format!("key-{n:03}")).collect(),
        },
    ]
}

/// `POST /tags/{arn}` — `TagResource`, as the WHOLE response, so the status, the body
/// bytes and the headers can all be asserted.
async fn tag_http(
    em: &Emulator,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> http::Response<bytes::Bytes> {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    let body = json!({ "tags": Value::Object(map) });
    call_http(em, "POST", &format!("/tags/{}", label(arn)), Some(&body)).await
}

/// `DELETE /tags/{arn}?tagKeys=…` — `UntagResource`, as the whole response. `tagKeys` is
/// an `@httpQuery` list, so each key travels as its own repeated parameter.
async fn untag_http(em: &Emulator, arn: &str, names: &[String]) -> http::Response<bytes::Bytes> {
    let query: Vec<String> = names
        .iter()
        .map(|name| format!("tagKeys={}", query_value(name)))
        .collect();
    call_http(
        em,
        "DELETE",
        &format!("/tags/{}?{}", label(arn), query.join("&")),
        None,
    )
    .await
}

/// The length a no-content answer states for the body it does not carry. An answer served
/// over HTTP/1.1 reaches the client without the header — the transport's own writer drops
/// `content-length` from a 204 — so this is the framing the in-process response carries.
const NO_CONTENT_LENGTH: &str = "0";

/// Assert one mutation answered the no-content success: the status, zero body bytes, and
/// the length such an answer states.
fn assert_no_content(case: &str, operation: &str, response: &http::Response<bytes::Bytes>) {
    let status = response.status().as_u16();
    let body = response.body();
    assert_eq!(
        status,
        TAG_MUTATION_SUCCESS_STATUS,
        "case {case:?}/{operation}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {status} / {:?}",
        String::from_utf8_lossy(body)
    );
    assert!(
        body.is_empty(),
        "case {case:?}/{operation}: a no-content answer carries no body bytes, got {:?}",
        String::from_utf8_lossy(body)
    );
    let framing = response
        .headers()
        .get("content-length")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
    assert_eq!(
        framing.as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}/{operation}: a no-content answer states the length it has, got {framing:?}"
    );
}

#[tokio::test]
async fn every_successful_tag_mutation_answers_no_content() {
    let table = status_table();
    assert!(
        table.len() >= 5,
        "the status table must span the class, got {} rows",
        table.len()
    );

    // One account per row: the published per-account email-contact quota bounds how many
    // contacts one account may hold at a time, and these rows are about the status a
    // mutation answers rather than about that quota.
    for (index, case) in table.iter().enumerate() {
        let em = emulator();
        let arn = create_tagged(&em, &format!("status-{index}"), Some(&case.stored)).await;

        // Repeated twice: the status does not depend on whether the mutation changed
        // anything, so the second, no-op call answers the same way.
        for attempt in 1..=2 {
            let response = tag_http(&em, &arn, &case.written).await;
            assert_no_content(case.what, &format!("TagResource attempt {attempt}"), &response);
        }

        // The write landed, which is what makes the status an answer about a real
        // mutation rather than a refusal dressed as a success.
        let mut expected = case.stored.clone();
        expected.extend(case.written.clone());
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported, expected,
            "case {:?}: the write must persist before its status means anything, got {reported:?}",
            case.what
        );

        for attempt in 1..=2 {
            let response = untag_http(&em, &arn, &case.removed).await;
            assert_no_content(
                case.what,
                &format!("UntagResource attempt {attempt}"),
                &response,
            );
        }

        for key in &case.removed {
            expected.remove(key);
        }
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported, expected,
            "case {:?}: the removal must persist too, got {reported:?}",
            case.what
        );
    }
}

/// The no-content status belongs to the bodyless tag mutations: every other operation of
/// this service answers its own success status, so the change is not a blanket 204.
#[tokio::test]
async fn every_other_operation_keeps_its_own_success_status() {
    let em = emulator();
    let arn = create_ok(&em, "contrast", "contrast@example.com").await;
    assert_no_content(
        "the mutation the contrast is drawn against",
        "TagResource",
        &tag_http(&em, &arn, &tag_map([pair("team", "infra")])).await,
    );

    // Rows differ in the kind of answer: a create whose `@http` code the model states
    // (201), the readers whose output carries members (200), and the bodyless successes
    // this service answers with the protocol default (200) — `SendActivationCode`,
    // `ActivateEmailContact` and `DeleteEmailContact`, whose wire form is pinned by
    // `email_contact_timestamp_wire_form.rs` and `email_contacts.rs`.
    let (status, body) = create(
        &em,
        &json!({ "name": "contrast-create", "emailAddress": "contrast-create@example.com" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"CreateEmailContact\": the status the model states explicitly, got {status} / {body:?}"
    );
    assert!(
        body.get("arn").is_some(),
        "case \"CreateEmailContact\": an answer carrying members must carry them, got {body:?}"
    );

    for (case, (status, body)) in [
        ("GetEmailContact", get(&em, &arn).await),
        ("ListEmailContacts", list(&em, "").await),
        ("ListTagsForResource", list_tags(&em, &arn).await),
    ] {
        assert_eq!(
            status, 200,
            "case {case:?}: an answer carrying members keeps its own 200, got {status} / {body:?}"
        );
        assert!(
            body.is_object(),
            "case {case:?}: an answer carrying members must carry a body, got {body:?}"
        );
    }

    let (status, body) = send_code(&em, &arn).await;
    assert_eq!(
        status, 200,
        "case \"SendActivationCode\": this service's other bodyless successes keep the \
         protocol default, got {status} / {body:?}"
    );
    let code = issued_code(&em, &arn).expect("the send must have issued a code");
    let (status, body) = activate(&em, &arn, &code).await;
    assert_eq!(
        status, 200,
        "case \"ActivateEmailContact\": this service's other bodyless successes keep the \
         protocol default, got {status} / {body:?}"
    );
    let (status, body) = delete(&em, &arn).await;
    assert_eq!(
        status, 200,
        "case \"DeleteEmailContact\": this service's other bodyless successes keep the \
         protocol default, got {status} / {body:?}"
    );

    // And a tag mutation that FAILS answers its modelled error status, not the
    // no-content success: the narrowing is about the success alone.
    let (status, body) = call(
        &em,
        "POST",
        &format!("/tags/{}", label(&arn)),
        Some(&json!({ "tags": { "team": "ghost" } })),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"TagResource on a deleted contact\": a refusal keeps its modelled status, \
         got {status} / {body:?}"
    );
    assert!(
        body.is_object(),
        "case \"TagResource on a deleted contact\": a refusal carries its error shape, got {body:?}"
    );
}
