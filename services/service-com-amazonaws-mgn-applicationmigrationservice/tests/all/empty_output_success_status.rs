// SPDX-License-Identifier: CC0-1.0

//! How this service concludes a success that carries no entity, end to end over HTTP.
//!
//! The rule under test: an operation whose model declares no output shape at all — its
//! output is `smithy.api#Unit`, so there is nothing for a caller to read — concludes
//! success with 204 No Content, a body of zero bytes and the `content-length: 0` that
//! states the length such a message has; it is never the 200 the protocol falls back to
//! when the `@http` code is left defaulted. An operation whose model NAMES a response
//! shape answers the status the model states for it, whatever that shape's members.
//!
//! The two output-less operations this crate implements are the tag mutations
//! (`smithy_query(query="digest")`: `TagResource` and `UntagResource` are the only bound
//! operations with `output: Unit` besides `DeleteConnector` and `DeleteVcenterClient`,
//! whose code the model states as 204 and which are still stubs). So the first two tables
//! drive the tag mutations across the class their answer must hold over — every taggable
//! resource kind, and every tag set and key set a write or a removal may carry — and the
//! third pins the contrast: the model-stated statuses of the operations whose answers DO
//! name a shape, which neither drift to 204 nor lose their body.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same restJson1
//! deserialize → handle → serialize path a client drives, and reads the RAW response bytes
//! and headers, so "no content" is checked as the absence of a body rather than as a parse.
//! (Over HTTP/1.1 the framing header does not reach the client: the HTTP writer the
//! vendored transport uses removes `content-length` from a 204, which is below anything
//! this crate decides — hence the in-process front door here.)

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::application_lifecycle::{Mgn, EMULATOR_ACCOUNT, REGION};
use crate::resource_tags::{emulator_with_one_of_each_kind, label, tags, write_cases};

/// The status an answer that carries no entity concludes with, and the length it states.
const NO_CONTENT: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// The modelled `TagKey` maximum length
/// (`smithy_query(query="shape",shape="TagKeys")`: member `TagKey`, `@length` 0..256).
const MAX_TAG_KEY_LEN: usize = 256;

/// One answer, as the wire carries it: the status, what it states its length to be, and
/// the bytes it carries.
#[derive(Debug)]
struct Answer {
    status: u16,
    content_length: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The body as text, for a message naming what actually arrived.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One request through the router, as an [`Answer`].
async fn answer(mgn: &Mgn, method: &str, path: &str, body: Option<&Value>) -> Answer {
    let (status, headers, raw) = mgn.call_http_stating_headers(method, path, body).await;
    Answer {
        status,
        content_length: headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .map(|(_, value)| value.clone()),
        body: raw,
    }
}

/// `POST /tags/{resourceArn}` with the pairs in the body — `TagResource`.
async fn tag_write(mgn: &Mgn, resource_arn: &str, pairs: &BTreeMap<String, String>) -> Answer {
    answer(
        mgn,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(&json!({"tags": pairs})),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an `@httpQuery`
/// list, so one parameter per key.
async fn tag_removal(mgn: &Mgn, resource_arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect::<Vec<_>>()
        .join("&");
    answer(
        mgn,
        "DELETE",
        &format!("/tags/{}?{query}", label(resource_arn)),
        None,
    )
    .await
}

/// `POST /<operation>` with `request`.
async fn post(mgn: &Mgn, operation: &str, request: &Value) -> Answer {
    answer(mgn, "POST", &format!("/{operation}"), Some(request)).await
}

/// Assert `got` is a no-content answer: the status, no bytes at all, and the stated length.
fn assert_no_content(case: &str, operation: &str, got: &Answer) {
    assert_eq!(
        got.status, NO_CONTENT,
        "case {case:?}: {operation} expected {NO_CONTENT}, got {} with body {:?}",
        got.status,
        got.text()
    );
    assert!(
        got.body.is_empty(),
        "case {case:?}: {operation} answered {NO_CONTENT} and so must carry no body, got \
         {:?}",
        got.text()
    );
    assert_eq!(
        got.content_length.as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: {operation} must state content-length {NO_CONTENT_LENGTH}, got {:?}",
        got.content_length
    );
}

/// The key sets a removal may carry: one key the resource holds, every key it holds, a key
/// it never held, a mix of both, a key at the modelled maximum length, and keys carrying
/// punctuation. Each is paired with the write that must precede it, so the removal is
/// exercised against both a present and an absent key.
fn removal_cases() -> Vec<(&'static str, BTreeMap<String, String>, Vec<String>)> {
    vec![
        (
            "one key the resource holds",
            tags(&[("stage", "test")]),
            vec!["stage".to_string()],
        ),
        (
            "every key the resource holds",
            tags(&[("stage", "test"), ("team", "network"), ("wave", "1")]),
            vec!["stage".to_string(), "team".to_string(), "wave".to_string()],
        ),
        (
            "a key the resource never held",
            tags(&[("stage", "test")]),
            vec!["never-set".to_string()],
        ),
        (
            "a present key and an absent one",
            tags(&[("stage", "test")]),
            vec!["stage".to_string(), "never-set".to_string()],
        ),
        (
            "a key at the modelled maximum length",
            BTreeMap::from([("k".repeat(MAX_TAG_KEY_LEN), "v".to_string())]),
            vec!["k".repeat(MAX_TAG_KEY_LEN)],
        ),
        (
            "keys carrying punctuation",
            tags(&[("cost-center", "4711"), ("owner.email", "ops@example")]),
            vec!["cost-center".to_string(), "owner.email".to_string()],
        ),
    ]
}

/// A tag WRITE concludes with no content, for every taggable resource kind and every tag
/// set the modelled map admits.
#[tokio::test]
async fn a_tag_write_concludes_with_no_content_for_every_kind_and_tag_set() {
    for (write_case, write) in write_cases() {
        let (mgn, resources) = emulator_with_one_of_each_kind().await;
        for resource in &resources {
            let case = format!("{:?} / {write_case}", resource.kind);
            assert_no_content(&case, "TagResource", &tag_write(&mgn, &resource.arn, &write).await);
            // A repeat of the same write stores nothing new and concludes the same way:
            // the answer is a fact about the operation's output, not about what changed.
            assert_no_content(
                &case,
                "the repeated TagResource",
                &tag_write(&mgn, &resource.arn, &write).await,
            );
        }
    }
}

/// A tag REMOVAL concludes with no content, for every taggable resource kind and every key
/// set — including a key the resource never carried, which removes nothing.
#[tokio::test]
async fn a_tag_removal_concludes_with_no_content_for_every_kind_and_key_set() {
    for (removal_case, write, keys) in removal_cases() {
        let (mgn, resources) = emulator_with_one_of_each_kind().await;
        for resource in &resources {
            let case = format!("{:?} / {removal_case}", resource.kind);
            assert_no_content(&case, "TagResource", &tag_write(&mgn, &resource.arn, &write).await);
            assert_no_content(
                &case,
                "UntagResource",
                &tag_removal(&mgn, &resource.arn, &keys).await,
            );
            // A second removal of the same keys is a no-op and concludes identically.
            assert_no_content(
                &case,
                "the repeated UntagResource",
                &tag_removal(&mgn, &resource.arn, &keys).await,
            );
        }
    }
}

/// The contrast: an operation whose model NAMES a response shape answers the status the
/// model states for it — the stated 204 where the shape carries no members, and the stated
/// 200 or 201 WITH a body where it does.
#[tokio::test]
async fn an_operation_whose_model_names_a_response_shape_keeps_its_stated_status() {
    let mgn = Mgn::new();

    // `InitializeService`: the model names `InitializeServiceResponse`, which carries no
    // members, and states `@http(code: 204)` for it — so its answer carries no content
    // either, reached from the model's own code rather than from an absent output shape.
    assert_no_content(
        "InitializeService",
        "InitializeService",
        &post(&mgn, "InitializeService", &json!({})).await,
    );

    // An operation whose output carries members answers the status the model states for
    // it, WITH a body: the status does not become 204 just because an answer may be short.
    let with_a_body: Vec<(&str, &str, Value, u16)> = vec![
        (
            "CreateApplication — the model states 201",
            "CreateApplication",
            json!({"name": "status-table-app", "tags": {"env": "dev"}}),
            201,
        ),
        (
            "ListApplications — the model states no code, so 200",
            "ListApplications",
            json!({}),
            200,
        ),
    ];
    let mut application_id = String::new();
    for (case, operation, request, expected) in with_a_body {
        let got = post(&mgn, operation, &request).await;
        assert_eq!(
            got.status,
            expected,
            "case {case:?}: expected {expected}, got {} with body {:?}",
            got.status,
            got.text()
        );
        assert!(
            !got.body.is_empty(),
            "case {case:?}: an output carrying members must arrive in a body, got none"
        );
        let parsed: Value = serde_json::from_slice(&got.body)
            .unwrap_or_else(|error| panic!("case {case:?}: the body is not JSON: {error}"));
        if operation == "CreateApplication" {
            application_id = parsed["applicationID"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no applicationID in {parsed:?}"))
                .to_string();
        }
    }

    // `ListTagsForResource`: its output carries the `tags` member, so a read of the tags
    // the create wrote answers 200 WITH a body — on the very route the two mutations
    // answer with no content on.
    let arn = format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:application/{application_id}");
    let (status, _, listed) = mgn
        .call_http("GET", &format!("/tags/{}", label(&arn)), None)
        .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200, got {status} with {listed:?}"
    );
    assert!(
        listed["tags"].is_object(),
        "ListTagsForResource must report a tags map in its body, got {listed:?}"
    );

    // `DeleteApplication`: the model names `DeleteApplicationResponse`, which carries no
    // members, and states 204 — the same no-content answer.
    assert_no_content(
        "DeleteApplication",
        "DeleteApplication",
        &post(
            &mgn,
            "DeleteApplication",
            &json!({"applicationID": application_id}),
        )
        .await,
    );
}

/// A REFUSAL of a tag mutation is not a no-content answer: the modelled error status and
/// the body stating it are untouched by what a SUCCESS of these operations concludes with.
#[tokio::test]
async fn a_refused_tag_mutation_still_answers_its_modelled_error_with_a_body() {
    let (mgn, _resources) = emulator_with_one_of_each_kind().await;
    let absent =
        format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:application/app-00000000000000000");

    for (operation, got) in [
        (
            "TagResource",
            tag_write(&mgn, &absent, &tags(&[("stage", "test")])).await,
        ),
        (
            "UntagResource",
            tag_removal(&mgn, &absent, &["stage".to_string()]).await,
        ),
    ] {
        assert_eq!(
            got.status,
            404,
            "{operation} on an ARN naming no resource expected 404, got {} with body {:?}",
            got.status,
            got.text()
        );
        assert!(
            !got.body.is_empty(),
            "{operation}: a refusal must carry its modelled error body, got none"
        );
    }
}
