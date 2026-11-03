// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the sequence-store slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the sequence-store helpers live here.
//!
//! The rules under test:
//!
//! * a sequence store is a resource of its own, stored once: whatever
//!   `CreateSequenceStore` carried is what the create's own response and a later
//!   `GetSequenceStore` report, each member taken from that one record and nothing
//!   invented for a member the request left out;
//!   * the store's id is the ten-digit decimal the modelled `SequenceStoreId` length
//!     admits and its ARN names it, both minted per store;
//!   * `eTagAlgorithmFamily` falls back to the value its documentation names as the
//!     default when the create names none, and is echoed when it does;
//!   * a replayed `clientToken` answers with the store the first call created rather
//!     than minting a second one, while a create that presents no token is a new store
//!     every time;
//! * a freshly created store owes the clock its move to `ACTIVE`, which a read observes
//!   but never causes;
//! * an id no store holds is the modelled 404, however the id is shaped.

use serde_json::{json, Value};
use service_com_amazonaws_omics_omics::state::{FIRST_SEQUENCE_STORE_ID, MAX_TAGS};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    assert_absent, assert_arn, assert_members, assert_timestamp, call, emulator, error_code,
    tag_map, POLL_INTERVAL, SETTLE_DEADLINE,
};

/// Every value the modelled `ETagAlgorithmFamily` enum admits.
/// Source: `smithy_query(query="shape",shape="ETagAlgorithmFamily")` -> MD5up | SHA256up
/// | SHA512up.
const E_TAG_ALGORITHM_FAMILIES: &[&str] = &["MD5up", "SHA256up", "SHA512up"];

/// The `ETagAlgorithmFamily` in force when a create names none.
/// Source: the `eTagAlgorithmFamily` member of `CreateSequenceStoreRequest` ("The default
/// value is MD5up").
const DEFAULT_E_TAG_ALGORITHM_FAMILY: &str = "MD5up";

/// The published upper bound of the modelled `propagatedSetLevelTags` list (len 0..50).
/// Source: the `propagatedSetLevelTags` member of `CreateSequenceStoreRequest`.
const PROPAGATED_SET_LEVEL_TAGS_BOUND: usize = 50;

/// The published bounds of the modelled `SequenceStoreId` length (10..36).
/// Source: the `id` member of `CreateSequenceStoreResponse`.
const SEQUENCE_STORE_ID_WIDTH: (usize, usize) = (10, 36);

/// A KMS key ARN satisfying the modelled `SseConfig$keyArn` (len 20..2048, an ARN with a
/// twelve-digit account).
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/omics-emulator-key";

/// `POST /sequencestore`.
async fn create_store(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/sequencestore", Some(body)).await
}

/// `GET /sequencestore/{id}`.
async fn get_store(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/sequencestore/{id}"), None).await
}

/// The `id` member of a 200 create response, after asserting the status.
fn created_store_id(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no id, got {body:?}"))
        .to_string();
    assert_sequence_store_id(case, &id);
    id
}

/// Assert `id` is the decimal the modelled `SequenceStoreId` describes — `^[0-9]+$` at
/// [`SEQUENCE_STORE_ID_WIDTH`] — and one this emulator could have minted.
fn assert_sequence_store_id(case: &str, id: &str) {
    let (min, max) = SEQUENCE_STORE_ID_WIDTH;
    assert!(
        id.len() >= min && id.len() <= max && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a {min}..{max} digit decimal id, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: id {id:?} does not parse as a number"));
    assert!(
        value >= FIRST_SEQUENCE_STORE_ID,
        "case {case:?}: expected an id at or above {FIRST_SEQUENCE_STORE_ID}, got {value}"
    );
}

/// Read the store until it reports `expected`, and answer that response.
///
/// Polling rather than sleeping a fixed span: the status is owed to the clock, so what is
/// asserted is that it arrives, not when a single read happened to land.
async fn await_store_status(router: &MultiServiceRouter, id: &str, expected: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = get_store(router, id).await;
        assert_eq!(
            status, 200,
            "sequence store {id}: expected 200, got {body:?}"
        );
        if body.get("status").and_then(Value::as_str) == Some(expected) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sequence store {id}: expected status {expected} within {SETTLE_DEADLINE:?}, \
             last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A list of `count` distinct tag keys, each inside the modelled `TagKey` length (1..128).
fn tag_keys(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|n| Value::String(format!("propagated{n}")))
            .collect(),
    )
}

/// A row of the create/read-back table: a request body, the members both the create and a
/// later read must report for it, and the members neither may carry.
struct StoreCase {
    /// How the row is reported in a failure, and the store's `clientToken`.
    case: &'static str,
    /// The request body, without the `name`/`clientToken` the row's `case` fills in.
    request: Value,
    /// Every member the responses must carry with exactly this value.
    expected: Value,
    /// Members the responses must NOT carry: nothing is invented for what the request
    /// left out.
    absent: Vec<&'static str>,
}

/// The table: one row per way a create can differ, so the rule is asserted over the class
/// and not over one request. The rows vary along the members present, the enum value
/// named, the size of the propagated-tag list and the form of the S3 locations.
fn store_cases() -> Vec<StoreCase> {
    let mut cases = vec![
        StoreCase {
            case: "only the required name",
            request: json!({}),
            expected: json!({
                "name": "only the required name",
                // The documented default, applied because the request named none.
                "eTagAlgorithmFamily": DEFAULT_E_TAG_ALGORITHM_FAMILY,
            }),
            absent: vec![
                "description",
                "sseConfig",
                "fallbackLocation",
                "s3Access",
                "propagatedSetLevelTags",
                "statusMessage",
                "updateTime",
            ],
        },
        StoreCase {
            case: "a described store",
            request: json!({ "description": "reads for the autumn cohort" }),
            expected: json!({
                "name": "a described store",
                "description": "reads for the autumn cohort",
                "eTagAlgorithmFamily": DEFAULT_E_TAG_ALGORITHM_FAMILY,
            }),
            absent: vec!["sseConfig", "fallbackLocation", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "a customer managed key",
            request: json!({ "sseConfig": { "type": "KMS", "keyArn": KEY_ARN } }),
            expected: json!({
                "name": "a customer managed key",
                "sseConfig": { "type": "KMS", "keyArn": KEY_ARN },
            }),
            absent: vec!["description", "fallbackLocation", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "a key-less encryption type",
            request: json!({ "sseConfig": { "type": "KMS" } }),
            expected: json!({
                "name": "a key-less encryption type",
                "sseConfig": { "type": "KMS" },
            }),
            absent: vec!["description", "fallbackLocation", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "a fallback location with a prefix",
            request: json!({ "fallbackLocation": "s3://omics-emulator-fallback/failed/" }),
            expected: json!({
                "name": "a fallback location with a prefix",
                "fallbackLocation": "s3://omics-emulator-fallback/failed/",
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "a bucket-only fallback location",
            request: json!({ "fallbackLocation": "s3://omics-emulator-fallback" }),
            expected: json!({
                "name": "a bucket-only fallback location",
                "fallbackLocation": "s3://omics-emulator-fallback",
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            // The modelled `FallbackLocation` pattern admits the empty string, which is
            // how a caller says "no fallback location" explicitly.
            case: "an empty fallback location",
            request: json!({ "fallbackLocation": "" }),
            expected: json!({
                "name": "an empty fallback location",
                "fallbackLocation": "",
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "an access log location",
            request: json!({
                "s3AccessConfig": { "accessLogLocation": "s3://omics-emulator-logs/access/" },
            }),
            expected: json!({
                "name": "an access log location",
                "s3Access": { "accessLogLocation": "s3://omics-emulator-logs/access/" },
            }),
            absent: vec!["description", "sseConfig", "fallbackLocation", "updateTime"],
        },
        StoreCase {
            case: "no propagated tags at all",
            request: json!({ "propagatedSetLevelTags": [] }),
            expected: json!({
                "name": "no propagated tags at all",
                "propagatedSetLevelTags": [],
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "one propagated tag",
            request: json!({ "propagatedSetLevelTags": ["propagated0"] }),
            expected: json!({
                "name": "one propagated tag",
                "propagatedSetLevelTags": ["propagated0"],
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            case: "propagated tags at the published bound",
            request: json!({
                "propagatedSetLevelTags": tag_keys(PROPAGATED_SET_LEVEL_TAGS_BOUND),
            }),
            expected: json!({
                "name": "propagated tags at the published bound",
                "propagatedSetLevelTags": tag_keys(PROPAGATED_SET_LEVEL_TAGS_BOUND),
            }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        },
        StoreCase {
            // `tags` is accepted by the create and reported by NEITHER response: no
            // `tags` member is modelled on either, so the store's own tags are read
            // through the tagging operations, not through these two.
            case: "tags at the documented bound",
            request: json!({ "tags": tag_map(MAX_TAGS) }),
            expected: json!({ "name": "tags at the documented bound" }),
            absent: vec!["tags", "description", "sseConfig", "updateTime"],
        },
        StoreCase {
            case: "every optional member at once",
            request: json!({
                "description": "everything named",
                "sseConfig": { "type": "KMS", "keyArn": KEY_ARN },
                "fallbackLocation": "s3://omics-emulator-fallback/all/",
                "eTagAlgorithmFamily": "SHA512up",
                "propagatedSetLevelTags": ["project", "cohort"],
                "s3AccessConfig": { "accessLogLocation": "s3://omics-emulator-logs/all/" },
                "tags": tag_map(3),
            }),
            expected: json!({
                "name": "every optional member at once",
                "description": "everything named",
                "sseConfig": { "type": "KMS", "keyArn": KEY_ARN },
                "fallbackLocation": "s3://omics-emulator-fallback/all/",
                "eTagAlgorithmFamily": "SHA512up",
                "propagatedSetLevelTags": ["project", "cohort"],
                "s3Access": { "accessLogLocation": "s3://omics-emulator-logs/all/" },
            }),
            absent: vec!["tags", "statusMessage", "updateTime"],
        },
    ];
    // One row per value the modelled enum admits, so the whole catalog is exercised and
    // not only the default.
    for family in E_TAG_ALGORITHM_FAMILIES {
        cases.push(StoreCase {
            case: family,
            request: json!({ "eTagAlgorithmFamily": family }),
            expected: json!({ "name": family, "eTagAlgorithmFamily": family }),
            absent: vec!["description", "sseConfig", "s3Access", "updateTime"],
        });
    }
    cases
}

/// The request body for a row: its own members plus the `name` and `clientToken` the row's
/// `case` names it by.
fn request_for(case: &StoreCase) -> Value {
    let mut body = case.request.as_object().cloned().unwrap_or_default();
    body.insert("name".to_string(), Value::String(case.case.to_string()));
    body.insert(
        "clientToken".to_string(),
        Value::String(format!("token-{}", case.case)),
    );
    Value::Object(body)
}

#[tokio::test]
async fn a_created_sequence_store_reads_back_from_state() {
    let router = emulator();
    let cases = store_cases();

    // Create them all first, so each read-back also proves the store keeps the rows apart
    // rather than answering with the most recent one.
    let mut created = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for store in &cases {
        let (status, body) = create_store(&router, &request_for(store)).await;
        let id = created_store_id(store.case, status, &body);
        let arn = body.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_arn(store.case, arn, "sequenceStore", &id);
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some("CREATING"),
            "case {:?}: a just-created sequence store must report the transient status, \
             got {body:?}",
            store.case
        );
        let creation_time = {
            assert_timestamp(store.case, "creationTime", &body);
            body.get("creationTime").cloned().unwrap_or(Value::Null)
        };
        // The create's own response already answers from the stored record.
        assert_members(store.case, "CreateSequenceStore", &store.expected, &body);
        assert_absent(store.case, &store.absent, &body);
        assert!(
            ids.insert(id.clone()),
            "case {:?}: id {id} was already handed out",
            store.case
        );
        created.push((id, arn.to_string(), creation_time));
    }

    for (store, (id, arn, creation_time)) in cases.iter().zip(&created) {
        // Read until the clock has settled the CREATING -> ACTIVE transition the create
        // owes, then assert the whole record that read reported.
        let body = await_store_status(&router, id, "ACTIVE").await;
        assert_members(store.case, "GetSequenceStore", &store.expected, &body);
        assert_absent(store.case, &store.absent, &body);
        assert_members(
            store.case,
            "GetSequenceStore",
            &json!({ "id": id, "arn": arn, "status": "ACTIVE", "creationTime": creation_time }),
            &body,
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_same_sequence_store() {
    let router = emulator();
    // The rule generalises over the token's text and over how much of the rest of the
    // request changes on the replay: the token alone decides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "identical replay",
            json!({ "clientToken": "store-1", "name": "alpha" }),
            json!({ "clientToken": "store-1", "name": "alpha" }),
        ),
        (
            "replay with different members",
            json!({ "clientToken": "store-2", "name": "beta", "description": "first" }),
            json!({ "clientToken": "store-2", "name": "beta-renamed", "eTagAlgorithmFamily": "SHA256up" }),
        ),
        (
            "replay of a token that is all punctuation",
            json!({ "clientToken": "::store-3::", "name": "gamma" }),
            json!({ "clientToken": "::store-3::", "name": "gamma" }),
        ),
    ];

    let mut first = Vec::new();
    for (case, body, _) in &rows {
        let (status, response) = create_store(&router, body).await;
        let id = created_store_id(case, status, &response);
        first.push((id, response));
    }

    for ((case, _, replay), (expected_id, expected_body)) in rows.iter().zip(&first) {
        let (status, body) = create_store(&router, replay).await;
        let id = created_store_id(case, status, &body);
        assert_eq!(
            &id, expected_id,
            "case {case:?}: a replayed clientToken must answer with the original store id, \
             got {body:?}"
        );
        for member in ["arn", "name", "description", "creationTime"] {
            assert_eq!(
                body.get(member),
                expected_body.get(member),
                "case {case:?}: a replayed clientToken must answer with the original \
                 {member}, got {body:?}"
            );
        }
    }

    // A create that presents no token is deduped against nothing: each one is its own
    // store, even when every other member is identical.
    let untokened = json!({ "name": "no token at all" });
    let (status, body) = create_store(&router, &untokened).await;
    let one = created_store_id("no token, first call", status, &body);
    let (status, body) = create_store(&router, &untokened).await;
    let two = created_store_id("no token, second call", status, &body);
    assert_ne!(
        one, two,
        "case \"no token at all\": two creates carrying no clientToken must be two stores, \
         got {one} twice"
    );
}

#[tokio::test]
async fn stores_sharing_a_name_are_distinct_resources() {
    let router = emulator();
    // `CreateSequenceStore` documents that "You can create multiple sequence stores per
    // region per account", and models no uniqueness on the name, so a reused name is a
    // second store with its own id, ARN and description.
    let name = "shared name";
    let mut seen = Vec::new();
    for (case, description) in [("first", "the first store"), ("second", "the second store")] {
        let (status, body) = create_store(
            &router,
            &json!({ "name": name, "description": description, "clientToken": case }),
        )
        .await;
        let id = created_store_id(case, status, &body);
        seen.push((case, id, description));
    }
    assert_ne!(
        seen[0].1, seen[1].1,
        "two stores sharing a name must be two resources, got {} twice",
        seen[0].1
    );
    for (case, id, description) in &seen {
        let body = await_store_status(&router, id, "ACTIVE").await;
        assert_members(
            case,
            "GetSequenceStore",
            &json!({ "id": id, "name": name, "description": description }),
            &body,
        );
    }
}

#[tokio::test]
async fn a_sequence_store_settles_to_active_on_the_clock() {
    let router = emulator();
    let (status, body) = create_store(&router, &json!({ "name": "clock" })).await;
    let id = created_store_id("clock", status, &body);

    // The transient status is observable: a read taken straight after the create still
    // sees it, so the span is never zero.
    let (_, before) = get_store(&router, &id).await;
    assert_eq!(
        before.get("status").and_then(Value::as_str),
        Some("CREATING"),
        "a store read before its transition is due must still be CREATING, got {before:?}"
    );

    let after = await_store_status(&router, &id, "ACTIVE").await;
    // Reading does not move it again, and does not restamp the creation time.
    let (_, again) = get_store(&router, &id).await;
    assert_eq!(
        again.get("status"),
        after.get("status"),
        "a second read must report the same status, got {again:?}"
    );
    assert_eq!(
        again.get("creationTime"),
        after.get("creationTime"),
        "a read must not restamp creationTime, got {again:?}"
    );
    assert_absent("clock", &["updateTime"], &again);
}

#[tokio::test]
async fn an_unknown_sequence_store_is_not_found() {
    let router = emulator();
    // One real store exists, so the 404s below are about the id, not about an empty
    // registry.
    let (status, body) = create_store(&router, &json!({ "name": "the only store" })).await;
    let existing = created_store_id("the only store", status, &body);

    // The rule generalises over the id's width and value, across the whole modelled
    // `SequenceStoreId` length (10..36): anything the registry does not hold is not
    // found, however it is shaped.
    let (min, max) = SEQUENCE_STORE_ID_WIDTH;
    let ids = [
        "0".repeat(min),
        "9".repeat(min),
        "1234567890".to_string(),
        "1".repeat(max),
        "9".repeat(max),
    ];
    for id in &ids {
        assert_ne!(
            id, &existing,
            "the fixture must not collide with the real id"
        );
        let (status, body) = get_store(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetSequenceStore, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn too_many_tags_on_a_sequence_store_are_refused() {
    let router = emulator();
    // The bound is MAX_TAGS, documented on the `tags` member ("You can configure up to 50
    // tags"), so the row at the bound is accepted and the row one past it is refused.
    for (case, count, expect_ok) in [
        ("at the tag bound", MAX_TAGS, true),
        ("one past the tag bound", MAX_TAGS + 1, false),
    ] {
        let (status, body) = create_store(
            &router,
            &json!({ "name": format!("tags-{count}"), "tags": tag_map(count) }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_propagated_tag_list_past_the_published_bound_is_refused() {
    let router = emulator();
    // The modelled `propagatedSetLevelTags` length is 0..50, so the list at the bound is
    // accepted and the one past it is refused by the centralized constraint walk.
    for (case, count, expect_ok) in [
        ("at the published bound", PROPAGATED_SET_LEVEL_TAGS_BOUND, true),
        (
            "one past the published bound",
            PROPAGATED_SET_LEVEL_TAGS_BOUND + 1,
            false,
        ),
    ] {
        let (status, body) = create_store(
            &router,
            &json!({
                "name": format!("propagated-{count}"),
                "propagatedSetLevelTags": tag_keys(count),
            }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
        }
    }
}
