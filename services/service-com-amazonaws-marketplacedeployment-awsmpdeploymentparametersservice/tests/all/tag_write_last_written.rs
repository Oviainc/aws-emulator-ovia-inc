// SPDX-License-Identifier: CC0-1.0

//! What a tag write owes the record it changed: the instant it was written.
//!
//! The rule under test: a stored deployment parameter's "last written" instant
//! (`DeploymentParameterRecord::updated_at`) is the instant the record's stored
//! content was last written, so EVERY call whose success writes that content — the
//! two tagging writes as much as the write `PutDeploymentParameter` performs —
//! refreshes it from the instant its own call was served at, while a call that
//! only REPORTS the record, and a write the service refused, leave it where the
//! last write left it. The first-written instant (`created_at`) belongs to the
//! create alone and no later write moves it.
//!
//! The rows span what a tag write can be, along the axes the rule generalises
//! over: which operation writes (`TagResource` merging, `UntagResource` removing),
//! how many entries the write names (none, one, several), whether the write
//! CHANGES the map or lands on content the record already holds, and what the keys
//! and values look like (an ASCII key, a key and value outside ASCII, an empty
//! value) — because the rule is about the record having been written, not about
//! which tags the write carried. The second test spans the calls that must NOT
//! move the field: the reporting read, and the two kinds of refusal — one the
//! request earns on its own content ahead of the store, and one that is only
//! decided at the record itself (the per-resource tag cap), which is the one that
//! proves the stamp is applied on success rather than on arrival.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    service_state, MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, list_tags, provision, put, tag, tag_map, untag, CATALOG,
};

/// The tags every record below is created with: two keys, so a removal can take
/// one of them, all of them, or none of them.
const STORED_TAGS: &[(&str, &str)] = &[("env", "dev"), ("extra", "x")];

/// One write under test, as the operation that performs it.
#[derive(Debug)]
enum Write {
    /// `TagResource` with this tag map.
    Tag(BTreeMap<String, String>),
    /// `UntagResource` with these tag keys.
    Untag(Vec<String>),
}

/// One call under test: the reporting read, or a write.
#[derive(Debug)]
enum Call {
    /// `ListTagsForResource`.
    Read,
    /// One of the two writes.
    Write(Write),
}

/// Perform `call` against `resource_arn` and report its status and body.
async fn perform(router: &MultiServiceRouter, resource_arn: &str, call: &Call) -> (u16, Value) {
    match call {
        Call::Read => list_tags(router, resource_arn).await,
        Call::Write(Write::Tag(tags)) => tag(router, resource_arn, tags).await,
        Call::Write(Write::Untag(tag_keys)) => untag(router, resource_arn, tag_keys).await,
    }
}

/// The `(created_at, updated_at)` the store holds for `resource_arn` — the record's
/// first-written and last-written instants, read out of the store itself because no
/// operation of this model reports either on the wire.
///
/// `pub(crate)` because the sibling `clock_sample_under_the_store_lock` suite reads
/// the same two instants: ONE reader, so the two suites cannot come to read the
/// field differently.
pub(crate) fn written_instants(engine: &engine::InMemoryEngine, resource_arn: &str) -> (SystemTime, SystemTime) {
    let state = service_state(engine).expect("the shared engine must hold this service's store");
    let store = state.lock();
    let record = store
        .parameters
        .get(resource_arn)
        .unwrap_or_else(|| panic!("the store must hold the record at {resource_arn}"));
    (record.created_at, record.updated_at)
}

/// One stored deployment parameter: the ARN the service minted for it, and what it
/// takes to write the SAME identity again.
struct Stored {
    resource_arn: String,
    product_id: String,
    body: Value,
}

/// Store one deployment parameter under its own identity, tagged with
/// [`STORED_TAGS`].
async fn store_parameter(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    index: usize,
) -> Stored {
    let product_id = format!("prod-written-{index}");
    let agreement_id = format!("agmt-written-{index}");
    provision(engine, CATALOG, &agreement_id, &[&product_id]);
    let body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": format!("Parameter-{index}"), "secretString": "s3cr3t" },
        "tags": STORED_TAGS
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>(),
    });
    let (status, created) = put(router, CATALOG, &product_id, &body).await;
    assert_eq!(
        status, 200,
        "case {index}: the setup Put must succeed, got {status} {created:?}"
    );
    let resource_arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {index}: expected a minted resourceArn, got {created:?}"))
        .to_string();
    Stored {
        resource_arn,
        product_id,
        body,
    }
}

#[tokio::test]
async fn every_tag_write_refreshes_the_records_last_written_instant() {
    let (router, engine) = emulator();

    let rows: Vec<(&str, Write)> = vec![
        (
            "a merge of a key the record does not hold",
            Write::Tag(tag_map(&[("owner", "platform")])),
        ),
        (
            "a merge giving a key the record holds a new value",
            Write::Tag(tag_map(&[("env", "prod")])),
        ),
        (
            "a merge of the value the record already holds",
            Write::Tag(tag_map(&[STORED_TAGS[0]])),
        ),
        (
            "a merge of several keys at once",
            Write::Tag(tag_map(&[("a", "1"), ("b", "2"), ("c", "3")])),
        ),
        (
            // This operation's `tags` target the map the model leaves unconstrained,
            // so a key and a value outside ASCII and an empty value are ordinary
            // content (`unconstrained_tag_characters` owns that rule).
            "a merge of a key and value outside ASCII, and an empty value",
            Write::Tag(tag_map(&[("tëam", "platförm"), ("note", "")])),
        ),
        (
            "a tags member carrying no entries at all",
            Write::Tag(BTreeMap::new()),
        ),
        (
            "a removal of a key the record holds",
            Write::Untag(vec![STORED_TAGS[0].0.to_string()]),
        ),
        (
            "a removal of a key the record does not hold",
            Write::Untag(vec!["absent".to_string()]),
        ),
        (
            "a removal of every key the record holds",
            Write::Untag(
                STORED_TAGS
                    .iter()
                    .map(|(key, _)| (*key).to_string())
                    .collect(),
            ),
        ),
        (
            "a removal naming the same key twice",
            Write::Untag(vec![
                STORED_TAGS[1].0.to_string(),
                STORED_TAGS[1].0.to_string(),
            ]),
        ),
    ];

    for (index, (case, write)) in rows.into_iter().enumerate() {
        let resource_arn = store_parameter(&router, &engine, index).await.resource_arn;
        let (created_at, created_updated_at) = written_instants(&engine, &resource_arn);

        let before = SystemTime::now();
        let (status, answered) = perform(&router, &resource_arn, &Call::Write(write)).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204, got {status} {answered:?}"
        );

        let (created_at_after, updated_at) = written_instants(&engine, &resource_arn);
        assert!(
            updated_at >= before && updated_at <= after,
            "case {case:?}: expected the last-written instant inside the write's own \
             window {before:?}..={after:?}, got {updated_at:?}"
        );
        assert!(
            updated_at > created_updated_at,
            "case {case:?}: expected the last-written instant to move past the create's \
             {created_updated_at:?}, got {updated_at:?}"
        );
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected the first-written instant to stay {created_at:?}, \
             got {created_at_after:?}"
        );
    }
}

#[tokio::test]
async fn the_three_writes_of_one_record_each_refresh_its_last_written_instant() {
    // The rule is about a write, not about which operation performs it: the
    // in-place rewrite `PutDeploymentParameter` performs for an identity already
    // stored, a merge and a removal are the three writes this model has for one
    // record, and each of them must leave the field at its own call's instant while
    // the first-written instant stays where the create put it.
    let (router, engine) = emulator();
    let stored = store_parameter(&router, &engine, 0).await;
    let (created_at, mut last_written) = written_instants(&engine, &stored.resource_arn);

    let mut observe = |case: &str, instants: (SystemTime, SystemTime), window: (SystemTime, SystemTime)| {
        let (created_at_after, updated_at) = instants;
        let (before, after) = window;
        assert!(
            updated_at >= before && updated_at <= after,
            "case {case:?}: expected the last-written instant inside the write's own \
             window {before:?}..={after:?}, got {updated_at:?}"
        );
        assert!(
            updated_at > last_written,
            "case {case:?}: expected the last-written instant to move past the previous \
             write's {last_written:?}, got {updated_at:?}"
        );
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected the first-written instant to stay {created_at:?}, \
             got {created_at_after:?}"
        );
        last_written = updated_at;
    };

    let before = SystemTime::now();
    let (status, answered) = put(&router, CATALOG, &stored.product_id, &stored.body).await;
    let after = SystemTime::now();
    assert_eq!(
        status, 200,
        "case \"an in-place rewrite of the same identity\": expected HTTP 200, got \
         {status} {answered:?}"
    );
    observe(
        "an in-place rewrite of the same identity",
        written_instants(&engine, &stored.resource_arn),
        (before, after),
    );

    let before = SystemTime::now();
    let (status, answered) = tag(
        &router,
        &stored.resource_arn,
        &tag_map(&[("owner", "platform")]),
    )
    .await;
    let after = SystemTime::now();
    assert_eq!(
        status, 204,
        "case \"a merge onto the rewritten record\": expected HTTP 204, got {status} {answered:?}"
    );
    observe(
        "a merge onto the rewritten record",
        written_instants(&engine, &stored.resource_arn),
        (before, after),
    );

    let before = SystemTime::now();
    let (status, answered) = untag(
        &router,
        &stored.resource_arn,
        &[STORED_TAGS[0].0.to_string()],
    )
    .await;
    let after = SystemTime::now();
    assert_eq!(
        status, 204,
        "case \"a removal from the merged record\": expected HTTP 204, got {status} {answered:?}"
    );
    observe(
        "a removal from the merged record",
        written_instants(&engine, &stored.resource_arn),
        (before, after),
    );
}

#[tokio::test]
async fn a_read_or_a_refused_write_leaves_the_last_written_instant_alone() {
    let (router, engine) = emulator();

    // A tag key in the namespace AWS reserves for its own generated tags: refused on
    // the request's own content, ahead of the store (`reserved_tag_keys` owns that
    // rule).
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");
    // A map of exactly the per-resource cap: admitted as an input, then refused at
    // the record, because merging it onto the keys the record already holds would
    // carry the resource past the cap (`tag_input_judged_before_lookup` owns the
    // split between the two judgements).
    let onto_the_cap: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), index.to_string()))
        .collect();

    let rows: Vec<(&str, Call, u16, Option<&str>)> = vec![
        ("a reporting read of the tag map", Call::Read, 200, None),
        (
            "a merge naming a key in the reserved namespace",
            Call::Write(Write::Tag(
                [(reserved.clone(), "stack".to_string())]
                    .into_iter()
                    .collect(),
            )),
            400,
            Some("ValidationException"),
        ),
        (
            "a removal naming a key in the reserved namespace",
            Call::Write(Write::Untag(vec![reserved.clone()])),
            400,
            Some("ValidationException"),
        ),
        (
            "a merge that would carry the record past the per-resource tag cap",
            Call::Write(Write::Tag(onto_the_cap)),
            400,
            // The tagging front end's own count refusal, not this service's modelled
            // input error (`tag_count_front_end` owns that rule).
            Some("BadRequestException"),
        ),
    ];

    for (index, (case, call, expected_status, expected_code)) in rows.into_iter().enumerate() {
        let resource_arn = store_parameter(&router, &engine, index).await.resource_arn;
        let (created_at, updated_at) = written_instants(&engine, &resource_arn);

        let (status, answered) = perform(&router, &resource_arn, &call).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answered:?}"
        );
        if let Some(code) = expected_code {
            assert_eq!(
                error_code(&answered),
                code,
                "case {case:?}: expected code {code}, got {answered:?}"
            );
        }

        let (created_at_after, updated_at_after) = written_instants(&engine, &resource_arn);
        assert_eq!(
            updated_at_after, updated_at,
            "case {case:?}: expected the last-written instant to stay {updated_at:?}, \
             got {updated_at_after:?}"
        );
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected the first-written instant to stay {created_at:?}, \
             got {created_at_after:?}"
        );
    }
}
