// SPDX-License-Identifier: CC0-1.0

//! RULE: a tag MUTATION of this service reports an accepted request with the status alone —
//! HTTP 204, with no entity body at all.
//!
//! `TagResource` and `UntagResource` are the two operations of the tag frontend whose
//! modelled output structure declares no member, so a caller has nothing to parse and the
//! status is the whole answer. Everything this service serves ITSELF keeps the status its
//! model leaves defaulted (HTTP 200) — including the operations whose output document is
//! empty (`CancelJob`, `DeleteSchedulingPolicy`, `DeregisterJobDefinition`,
//! `DeleteConsumableResource`, …) and the tag plane's own READER, whose answer carries the
//! tags map. The second test below is that boundary.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is what a `batch` client reads: the status, the absence of a document,
//! and the length such an answer states for the body it does not carry.
//!
//! The rows of the first table differ along what the rule generalises over — which of the
//! two mutations it is, how many tags or keys travel in one request, the character classes
//! of a key and a value, a key the resource already holds, a key it does not hold, and the
//! fifty tags one resource may hold — and over EVERY resource kind the mutation can name.
//! Each row is also read back through `ListTagsForResource`, so a no-content success is
//! never reported for a mutation the store did not take.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::scaffolded::TAGGING_NO_CONTENT_STATUS;
use service_com_amazonaws_batch_awsbatchv20160810::state::limits;

use crate::job_lifecycle::{
    compute_environment, container_properties, job_definition, job_queue, Batch,
};
use crate::resource_tags::{listed, tag_keys_query, tags_body, tags_uri};

/// The status a `batch` client reads off an accepted tag mutation, and the length such an
/// answer states for the body it does not carry.
///
/// Written out as the LITERALS a client sees rather than taken from the crate's own
/// constant, so changing that constant fails this suite instead of moving with it; the
/// assertion in the first test below is what ties the two together.
pub(crate) const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// The status every operation this service serves ITSELF answers a success with, document
/// or no document.
const BODY_BEARING_SUCCESS_STATUS: u16 = 200;

/// Which tag mutation a row drives, and what it carries.
enum Mutation {
    /// `TagResource`: the `tags` map the request writes.
    Tag(Value),
    /// `UntagResource`: the `tagKeys` the request removes.
    Untag(Vec<String>),
}

/// One row of the table: what makes it a different KIND of mutation, the mutation itself,
/// and the tag map the resource holds once it is taken.
struct Row {
    label: &'static str,
    mutation: Mutation,
    expected: BTreeMap<String, String>,
}

/// `pairs` as an ordered map, for the expectation a row reads back.
fn map_of(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// `count` tags named `prefix-<n>`.
fn pairs(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}-{index}"), index.to_string()))
        .collect()
}

/// The mutations every resource kind is driven through, in order, with the map each one
/// leaves behind.
///
/// The last tag row fills the resource to the `MAX_TAGS_PER_RESOURCE` tags it may hold, so
/// the table spans a request carrying one tag, several, and the whole ceiling's worth.
fn rows() -> Vec<Row> {
    let mut held: Vec<(String, String)> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();

    let tag_row = |label: &'static str,
                       written: Vec<(String, String)>,
                       held: &mut Vec<(String, String)>| {
        for (key, value) in &written {
            match held.iter_mut().find(|(stored, _)| stored == key) {
                Some(slot) => slot.1 = value.clone(),
                None => held.push((key.clone(), value.clone())),
            }
        }
        Row {
            label,
            mutation: Mutation::Tag(tags_body(&written)),
            expected: map_of(held),
        }
    };

    rows.push(tag_row(
        "one ASCII tag",
        vec![("team".to_string(), "batch".to_string())],
        &mut held,
    ));
    rows.push(tag_row("five tags in one request", pairs("bulk", 5), &mut held));
    rows.push(tag_row(
        "a key the resource already holds, replaced",
        vec![("team".to_string(), "platform".to_string())],
        &mut held,
    ));
    rows.push(tag_row(
        "an empty value, the lower bound TagValue admits",
        vec![("empty".to_string(), String::new())],
        &mut held,
    ));
    rows.push(tag_row(
        "a non-ASCII key and value",
        vec![("キー".to_string(), "値-🚀".to_string())],
        &mut held,
    ));
    let remaining = limits::MAX_TAGS_PER_RESOURCE - held.len();
    rows.push(tag_row(
        "enough more tags to fill the resource to the tags it may hold",
        pairs("fill", remaining),
        &mut held,
    ));

    let untag_row = |label: &'static str,
                         removed: Vec<String>,
                         held: &mut Vec<(String, String)>| {
        held.retain(|(key, _)| !removed.contains(key));
        Row {
            label,
            mutation: Mutation::Untag(removed),
            expected: map_of(held),
        }
    };

    rows.push(untag_row("one key", vec!["team".to_string()], &mut held));
    rows.push(untag_row(
        "three keys at once",
        vec![
            "bulk-0".to_string(),
            "bulk-1".to_string(),
            "bulk-2".to_string(),
        ],
        &mut held,
    ));
    rows.push(untag_row(
        "a key the resource does not hold",
        vec!["never-written".to_string()],
        &mut held,
    ));
    let every_remaining: Vec<String> = held.iter().map(|(key, _)| key.clone()).collect();
    rows.push(untag_row(
        "every key the resource still holds",
        every_remaining,
        &mut held,
    ));
    rows
}

/// Drive one mutation against `resource_arn`; answer the status, the length the response
/// states, and the decoded document (`Value::Null` when the response carries no body at
/// all).
async fn mutate(
    batch: &Batch,
    resource_arn: &str,
    mutation: &Mutation,
) -> (u16, Option<String>, Value) {
    let (method, uri, payload) = match mutation {
        Mutation::Tag(tags) => (
            "POST",
            tags_uri(resource_arn),
            Some(json!({"tags": tags.clone()})),
        ),
        Mutation::Untag(keys) => (
            "DELETE",
            format!("{}?{}", tags_uri(resource_arn), tag_keys_query(keys)),
            None,
        ),
    };
    let (status, headers, body) = batch.send_reported(method, &uri, payload).await;
    let length = headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (status, length, body)
}

/// Assert `case` was answered with no content: the no-content status, no document, and the
/// length an absent body has.
fn assert_no_content(case: &str, status: u16, length: Option<&str>, body: &Value) {
    assert_eq!(
        status, NO_CONTENT_STATUS,
        "case {case:?}: expected HTTP {NO_CONTENT_STATUS}, got {status} with the body {body}"
    );
    assert_eq!(
        body,
        &Value::Null,
        "case {case:?}: a no-content answer carries no document, got {body}"
    );
    assert_eq!(
        length,
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: expected the stated length {NO_CONTENT_LENGTH:?} of an absent body, got \
         {length:?}"
    );
}

/// One resource of every kind a tag mutation can name, as the kind's label and the ARN its
/// create minted.
async fn resources(batch: &Batch) -> Vec<(&'static str, String)> {
    let environment = compute_environment(batch, "no-content-env").await;
    let queue = job_queue(batch, "no-content-queue", &environment).await;
    let (definition, _revision) =
        job_definition(batch, "no-content-def", container_properties()).await;
    let policy = batch
        .ok(
            "/v1/createschedulingpolicy",
            json!({"name": "no-content-policy"}),
        )
        .await["arn"]
        .as_str()
        .expect("CreateSchedulingPolicy answers an ARN")
        .to_string();
    let consumable = batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "no-content-consumable", "totalQuantity": 4}),
        )
        .await["consumableResourceArn"]
        .as_str()
        .expect("CreateConsumableResource answers an ARN")
        .to_string();

    vec![
        ("compute environment", environment),
        ("job queue", queue),
        ("job definition", definition),
        ("scheduling policy", policy),
        ("consumable resource", consumable),
    ]
}

/// RULE: every accepted tag mutation is answered with no content — the no-content status and
/// no body — and the mutation it reports is the one the store took.
#[tokio::test]
async fn every_accepted_tag_mutation_answers_no_content() {
    // The status the handlers answer with is the status asserted here, so neither can drift
    // from the other without this failing.
    assert_eq!(
        TAGGING_NO_CONTENT_STATUS, NO_CONTENT_STATUS,
        "case the status the handlers answer with: expected {NO_CONTENT_STATUS}, got \
         {TAGGING_NO_CONTENT_STATUS}"
    );

    let batch = Batch::new();
    for (kind, resource_arn) in resources(&batch).await {
        for row in rows() {
            let case = format!("{kind}: {}", row.label);
            let (status, length, body) = mutate(&batch, &resource_arn, &row.mutation).await;
            assert_no_content(&case, status, length.as_deref(), &body);

            // The success is backed by the store: the plane's own reader reports exactly
            // the map the mutation leaves behind.
            let stored = listed(&batch, &case, &resource_arn).await;
            assert_eq!(
                stored, row.expected,
                "case {case:?}: expected the stored tags {:?}, got {stored:?}",
                row.expected
            );
        }
    }
}

/// RULE's boundary: the operations this service serves ITSELF answer a success with the
/// status their model leaves defaulted and the document rendered for it — the empty one
/// included — so only the tag mutations answer without content.
#[tokio::test]
async fn every_other_success_of_this_service_carries_its_document() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "document-env").await;
    let queue = job_queue(&batch, "document-queue", &environment).await;
    let (_definition, _revision) =
        job_definition(&batch, "document-def", container_properties()).await;
    batch
        .ok(
            "/v1/createschedulingpolicy",
            json!({"name": "document-policy"}),
        )
        .await;
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "document-consumable"}),
        )
        .await;
    let job_id = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "document-job",
                "jobQueue": "document-queue",
                "jobDefinition": "document-def",
            }),
        )
        .await["jobId"]
        .as_str()
        .expect("SubmitJob answers a job id")
        .to_string();

    // The mutations of this service whose own output structure declares no member: each
    // answers the empty JSON document at the status the model defaults to, which is what
    // makes the tag plane's no-content answer a difference a client can see.
    let empty_document: &[(&str, &str, Value)] = &[
        (
            "DeleteSchedulingPolicy",
            "/v1/deleteschedulingpolicy",
            json!({"arn": crate::job_lifecycle::arn("scheduling-policy/document-policy")}),
        ),
        (
            "DeregisterJobDefinition",
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": "document-def"}),
        ),
        (
            "DeleteConsumableResource",
            "/v1/deleteconsumableresource",
            json!({"consumableResource": "document-consumable"}),
        ),
        (
            "CancelJob",
            "/v1/canceljob",
            json!({"jobId": &job_id, "reason": "the boundary of the no-content rule"}),
        ),
    ];
    for (case, uri, payload) in empty_document {
        let (status, body) = batch.call(uri, payload.clone()).await;
        assert_eq!(
            status, BODY_BEARING_SUCCESS_STATUS,
            "case {case}: expected HTTP {BODY_BEARING_SUCCESS_STATUS}, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case}: expected the empty JSON document, got {body}"
        );
    }

    // The tag plane's own READER answers at the same status, with the map it reports: the
    // no-content framing is about an answer that has nothing to carry, not about the plane.
    let (status, body) = batch.send("GET", &tags_uri(&queue), None).await;
    assert_eq!(
        status, BODY_BEARING_SUCCESS_STATUS,
        "case ListTagsForResource: expected HTTP {BODY_BEARING_SUCCESS_STATUS}, got {status} \
         {body}"
    );
    assert!(
        body.get("tags").is_some(),
        "case ListTagsForResource: expected the tags map in the document, got {body}"
    );
}
