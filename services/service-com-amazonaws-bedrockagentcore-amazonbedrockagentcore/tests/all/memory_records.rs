// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore Memory long-term record surface:
//! `BatchCreateMemoryRecords`, `GetMemoryRecord` and `DeleteMemoryRecord`.
//!
//! The rules under test:
//!
//! * a batch create stores one record per entry the request listed, under a server-minted
//!   `memoryRecordId`, with the content, namespaces, timestamp, metadata and strategy
//!   association that entry carried, and reports one `SUCCEEDED` outcome per entry
//!   tracked by its `requestIdentifier`; a record expiry stated in days gives every
//!   record of the request an `expiresAt` that many days after its creation, and a
//!   request that states none (or the `REMOVE` action, which "does not apply when
//!   creating records") creates records that do not expire;
//! * a read reports a stored record from what was stored, and a read of a record this
//!   memory does not hold is refused `ResourceNotFoundException` (404);
//! * a delete removes the ONE record its `memoryRecordId` addresses and leaves every
//!   other record of that memory container readable;
//! * a replayed `clientToken` answers with the records the first batch created and
//!   stores no second set.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the wire shape (routes, status codes, `restJson1` bodies, modelled
//! error codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const MEMORY: &str = "record-memory-0123456789";
/// The same memory addressed by its ARN spelling, which that same pattern admits.
const MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/record-memory-0123456789";
/// A second, unrelated memory container.
const OTHER_MEMORY: &str = "other-memory-9876543210";

/// Seconds in one day, the unit `recordExpiryDurationDays` states an expiry in.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// The strategy association this emulator reports for a record created with no
/// `memoryStrategyId` (`crate::state::CALLER_MANAGED_MEMORY_STRATEGY_ID`).
const CALLER_MANAGED_STRATEGY: &str = "CUSTOM";

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does, so a memory ARN's
/// `/` reaches the server as part of the label rather than as URI syntax.
fn label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn batch_create(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/memoryRecords/batchCreate", label(memory)),
        Some(body),
    )
    .await
}

async fn get_record(router: &MultiServiceRouter, memory: &str, record: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/memories/{}/memoryRecord/{}",
            label(memory),
            label(record)
        ),
        None,
    )
    .await
}

async fn delete_record(router: &MultiServiceRouter, memory: &str, record: &str) -> (u16, Value) {
    send(
        router,
        "DELETE",
        &format!(
            "/memories/{}/memoryRecords/{}",
            label(memory),
            label(record)
        ),
        None,
    )
    .await
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// `MemoryRecordId`'s modelled pattern `^mem-[a-zA-Z0-9-_]*$` with its 40..50 length,
/// checked without a regex dependency.
fn is_modelled_record_id(id: &str) -> bool {
    (40..=50).contains(&id.len())
        && id.starts_with("mem-")
        && id[4..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One record-creation entry. `requestIdentifier` matches `^[a-zA-Z0-9_-]+$`, and
/// `namespaces` holds at most one namespace (`NamespacesList` @length max 1).
fn entry(request_identifier: &str, text: &str, namespace: &str) -> Value {
    json!({
        "requestIdentifier": request_identifier,
        "namespaces": [namespace],
        "content": {"text": text},
        "timestamp": 1_700_000_000,
    })
}

/// A record the server reports back, read through `GetMemoryRecord`.
async fn stored_record(router: &MultiServiceRouter, memory: &str, id: &str) -> Value {
    let (status, body) = get_record(router, memory, id).await;
    assert_eq!(
        status, 200,
        "reading back {id:?}: expected HTTP 200 from GetMemoryRecord, got {status} with {body:?}"
    );
    body["memoryRecord"].clone()
}

/// A batch create stores every entry it was given and a later `GetMemoryRecord` reports
/// each one from what was stored: the content, the namespaces, the timestamp, the
/// metadata and the strategy association the entry carried.
///
/// Rows differ along the entry count, the `MemoryRecordMetadataValue` union member
/// (source: smithy_query shape="MemoryRecordMetadataValue": stringValue |
/// stringListValue | numberValue | dateTimeValue), whether the entry named a
/// `memoryStrategyId`, the namespace form and the content size.
#[tokio::test]
async fn created_memory_records_round_trip_through_get_memory_record() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        ("one entry, no strategy, no metadata", json!([
            entry("req-1", "the user prefers window seats", "support/facts/actor-1"),
        ])),
        ("two entries, one strategy each", json!([
            {
                "requestIdentifier": "req-2a",
                "namespaces": ["support/preferences/actor-1"],
                "content": {"text": "likes aisle seats"},
                "timestamp": 1_700_000_100,
                "memoryStrategyId": "userPreference-abc123",
            },
            {
                "requestIdentifier": "req-2b",
                "namespaces": ["support/facts/actor-2"],
                "content": {"text": "lives in Berlin"},
                "timestamp": 1_700_000_200,
                "memoryStrategyId": "semantic_strategy-9",
            },
        ])),
        ("metadata: every union member, one per entry", json!([
            {
                "requestIdentifier": "req-3a",
                "namespaces": ["support/facts/actor-3"],
                "content": {"text": "string metadata"},
                "timestamp": 1_700_000_300,
                "metadata": {"topic": {"stringValue": "travel"}},
            },
            {
                "requestIdentifier": "req-3b",
                "namespaces": ["support/facts/actor-3"],
                "content": {"text": "string-list metadata"},
                "timestamp": 1_700_000_301,
                "metadata": {"tags": {"stringListValue": ["travel", "seats", "window"]}},
            },
            {
                "requestIdentifier": "req-3c",
                "namespaces": ["support/facts/actor-3"],
                "content": {"text": "number metadata"},
                "timestamp": 1_700_000_302,
                "metadata": {"confidence": {"numberValue": 0.5}},
            },
            {
                "requestIdentifier": "req-3d",
                "namespaces": ["support/facts/actor-3"],
                "content": {"text": "date-time metadata"},
                "timestamp": 1_700_000_303,
                "metadata": {"observedAt": {"dateTimeValue": 1_699_999_999}},
            },
        ])),
        ("a long content and a wildcard namespace", json!([
            {
                "requestIdentifier": "req_4",
                "namespaces": ["*"],
                "content": {"text": "x".repeat(4_000)},
                "timestamp": 1_700_000_400,
                "metadata": {
                    "k1": {"stringValue": "v1"},
                    "k2": {"numberValue": 42.0},
                },
            },
        ])),
    ];

    for (case, records) in cases {
        let (status, created) = batch_create(
            &router,
            MEMORY,
            json!({"records": records.clone()}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from BatchCreateMemoryRecords, got {status} with \
             {created:?}"
        );
        let successful = created["successfulRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let entries = records.as_array().expect("the case's entries").clone();
        assert_eq!(
            successful.len(),
            entries.len(),
            "case {case:?}: expected one successful record per entry ({}), got {:?}",
            entries.len(),
            created["successfulRecords"]
        );
        assert_eq!(
            created["failedRecords"],
            json!([]),
            "case {case:?}: expected no failed records, got {:?}",
            created["failedRecords"]
        );

        for (outcome, sent) in successful.iter().zip(entries.iter()) {
            let id = outcome["memoryRecordId"].as_str().unwrap_or_default();
            assert!(
                is_modelled_record_id(id),
                "case {case:?}: expected a server-minted memoryRecordId matching \
                 ^mem-[a-zA-Z0-9-_]*$ with length 40..50, got {id:?}"
            );
            assert_eq!(
                outcome["status"], "SUCCEEDED",
                "case {case:?}: expected status SUCCEEDED for {id:?}, got {:?}",
                outcome["status"]
            );
            assert_eq!(
                outcome["requestIdentifier"], sent["requestIdentifier"],
                "case {case:?}: expected the entry's requestIdentifier back, got {:?}",
                outcome["requestIdentifier"]
            );

            let record = stored_record(&router, MEMORY, id).await;
            assert_eq!(
                record["memoryRecordId"], outcome["memoryRecordId"],
                "case {case:?}: expected the read to report the created id, got {:?}",
                record["memoryRecordId"]
            );
            assert_eq!(
                record["content"], sent["content"],
                "case {case:?}: expected the stored content back unchanged, got {:?}",
                record["content"]
            );
            assert_eq!(
                record["namespaces"], sent["namespaces"],
                "case {case:?}: expected the stored namespaces back unchanged, got {:?}",
                record["namespaces"]
            );
            assert_eq!(
                record["timestamp"], sent["timestamp"],
                "case {case:?}: expected the entry's timestamp back, got {:?}",
                record["timestamp"]
            );
            assert_eq!(
                record["metadata"],
                sent.get("metadata").cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the stored metadata back unchanged, got {:?}",
                record["metadata"]
            );
            let expected_strategy = sent
                .get("memoryStrategyId")
                .and_then(Value::as_str)
                .unwrap_or(CALLER_MANAGED_STRATEGY);
            assert_eq!(
                record["memoryStrategyId"], expected_strategy,
                "case {case:?}: expected memoryStrategyId {expected_strategy:?}, got {:?}",
                record["memoryStrategyId"]
            );
            assert!(
                record["createdAt"].as_f64().is_some_and(|at| at > 0.0),
                "case {case:?}: expected a createdAt instant, got {:?}",
                record["createdAt"]
            );
            assert_eq!(
                record["expiresAt"],
                Value::Null,
                "case {case:?}: a record created with no record expiry does not expire, got {:?}",
                record["expiresAt"]
            );
        }
    }
}

/// `recordExpirySpecification` decides the records' `expiresAt`: a duration in days
/// gives every record of the request an expiry that many days after it was created,
/// while the `REMOVE` action — which "does not apply when creating records" — and an
/// absent specification leave the records without one.
///
/// Rows cover the published minimum of `recordExpiryDurationDays` (31), a value above
/// it, the one other union member (`action: REMOVE`, the sole `RecordExpiryAction`
/// value) and the absent specification.
#[tokio::test]
async fn a_record_expiry_in_days_sets_each_record_s_expiry() {
    let router = emulator();

    let cases: Vec<(&str, Value, Option<f64>)> = vec![
        ("no specification", Value::Null, None),
        ("the minimum, 31 days", json!({"recordExpiryDurationDays": 31}), Some(31.0)),
        ("90 days", json!({"recordExpiryDurationDays": 90}), Some(90.0)),
        ("3650 days", json!({"recordExpiryDurationDays": 3650}), Some(3650.0)),
        ("the REMOVE action", json!({"action": "REMOVE"}), None),
    ];

    for (case, specification, expected_days) in cases {
        let mut request = json!({
            "records": [entry("expiry-probe", "remember this", "support/facts/actor-1")],
        });
        if !specification.is_null() {
            request["recordExpirySpecification"] = specification.clone();
        }

        let (status, created) = batch_create(&router, MEMORY, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from BatchCreateMemoryRecords, got {status} with \
             {created:?}"
        );
        let id = created["successfulRecords"][0]["memoryRecordId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let record = stored_record(&router, MEMORY, &id).await;
        let created_at = record["createdAt"].as_f64().unwrap_or_default();

        match expected_days {
            None => assert_eq!(
                record["expiresAt"],
                Value::Null,
                "case {case:?}: expected no expiry on the record, got {:?}",
                record["expiresAt"]
            ),
            Some(days) => {
                let expires_at = record["expiresAt"].as_f64().unwrap_or_default();
                let expected = created_at.floor() + days * SECONDS_PER_DAY;
                assert!(
                    (expires_at - expected).abs() < 1.0,
                    "case {case:?}: expected an expiry {days} days after createdAt \
                     ({expected}), got {expires_at}"
                );
            }
        }
    }
}

/// A delete removes the ONE record its `memoryRecordId` addresses: that record is gone
/// (a later read is refused 404) and every other record created beside it is still
/// readable, whatever the batch size and whichever position the deleted record held.
#[tokio::test]
async fn deleting_one_memory_record_leaves_every_other_record() {
    // Rows differ along the batch size and the position of the record deleted from it.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("two records, delete the first", 2, 0),
        ("two records, delete the second", 2, 1),
        ("three records, delete the middle one", 3, 1),
        ("five records, delete the last", 5, 4),
        ("one record, delete it", 1, 0),
    ];

    for (case, count, position) in cases {
        let router = emulator();
        let records: Vec<Value> = (0..count)
            .map(|index| {
                entry(
                    &format!("req-{index}"),
                    &format!("fact number {index}"),
                    &format!("support/facts/actor-{index}"),
                )
            })
            .collect();
        let (status, created) = batch_create(&router, MEMORY, json!({"records": records})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from BatchCreateMemoryRecords, got {status} with \
             {created:?}"
        );
        let ids: Vec<String> = created["successfulRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|outcome| {
                outcome["memoryRecordId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids.len(),
            count,
            "case {case:?}: expected {count} created ids, got {ids:?}"
        );
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: expected every minted memoryRecordId to differ, got {ids:?}"
        );

        let removed = &ids[position];
        let (status, deleted) = delete_record(&router, MEMORY, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteMemoryRecord, got {status} with \
             {deleted:?}"
        );
        assert_eq!(
            deleted["memoryRecordId"], *removed,
            "case {case:?}: expected the deleted id back, got {:?}",
            deleted["memoryRecordId"]
        );

        let (status, body) = get_record(&router, MEMORY, removed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 reading the deleted record, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException reading the deleted record, got \
             {body:?}"
        );
        assert_eq!(
            body["message"], "MemoryItem does not exist",
            "case {case:?}: expected the modelled not-found message, got {:?}",
            body["message"]
        );

        // A second delete of the same record has nothing left to remove.
        let (status, body) = delete_record(&router, MEMORY, removed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 deleting the record twice, got {status} with \
             {body:?}"
        );

        for (index, id) in ids.iter().enumerate() {
            if index == position {
                continue;
            }
            let record = stored_record(&router, MEMORY, id).await;
            assert_eq!(
                record["content"],
                json!({"text": format!("fact number {index}")}),
                "case {case:?}: expected record {index} to survive the delete unchanged, got {record:?}"
            );
        }
    }
}

/// A record is addressed inside its own memory container: the id another container
/// minted is not found there, and the memory's ARN spelling addresses the same
/// container as its bare id.
#[tokio::test]
async fn a_memory_record_is_scoped_to_its_memory_container() {
    let router = emulator();

    let (status, created) = batch_create(
        &router,
        MEMORY,
        json!({"records": [entry("scoped", "a fact", "support/facts/actor-1")]}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"create in the first memory\": expected HTTP 201, got {status} with {created:?}"
    );
    let id = created["successfulRecords"][0]["memoryRecordId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The ARN spelling of the same memory addresses the same container.
    let (status, body) = get_record(&router, MEMORY_ARN, &id).await;
    assert_eq!(
        status, 200,
        "case \"read through the memory ARN\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["memoryRecord"]["memoryRecordId"], id,
        "case \"read through the memory ARN\": expected the same record, got {body:?}"
    );

    // A different container holds nothing under that id.
    let (status, body) = get_record(&router, OTHER_MEMORY, &id).await;
    assert_eq!(
        status, 404,
        "case \"read from another memory\": expected HTTP 404, got {status} with {body:?}"
    );
    let (status, body) = delete_record(&router, OTHER_MEMORY, &id).await;
    assert_eq!(
        status, 404,
        "case \"delete from another memory\": expected HTTP 404, got {status} with {body:?}"
    );

    // And an id nothing ever created is not found either.
    let unknown = format!("mem-{}", "0".repeat(40));
    for (case, status) in [
        ("read", get_record(&router, MEMORY, &unknown).await),
        ("delete", delete_record(&router, MEMORY, &unknown).await),
    ] {
        assert_eq!(
            status.0, 404,
            "case {case:?} of an id nothing created: expected HTTP 404, got {:?}",
            status
        );
        assert_eq!(
            error_code(&status.1),
            "ResourceNotFoundException",
            "case {case:?} of an id nothing created: expected ResourceNotFoundException, got {:?}",
            status.1
        );
    }
}

/// `BatchCreateMemoryRecords` is `@idempotent` with an `@idempotencyToken`: replaying a
/// token answers with the records that token already created and stores no second set,
/// while a different token creates new records.
#[tokio::test]
async fn a_replayed_client_token_creates_no_second_set_of_records() {
    let router = emulator();

    let records = json!([
        entry("token-a", "first fact", "support/facts/actor-1"),
        entry("token-b", "second fact", "support/facts/actor-2"),
    ]);
    let request = json!({"clientToken": "batch-token-1", "records": records.clone()});

    let (status, first) = batch_create(&router, MEMORY, request.clone()).await;
    assert_eq!(
        status, 201,
        "case \"first call\": expected HTTP 201, got {status} with {first:?}"
    );
    let (status, replay) = batch_create(&router, MEMORY, request).await;
    assert_eq!(
        status, 201,
        "case \"replayed token\": expected HTTP 201, got {status} with {replay:?}"
    );
    assert_eq!(
        replay["successfulRecords"], first["successfulRecords"],
        "case \"replayed token\": expected the records the first call created, got {:?}",
        replay["successfulRecords"]
    );

    let (status, fresh) = batch_create(
        &router,
        MEMORY,
        json!({"clientToken": "batch-token-2", "records": records}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"a different token\": expected HTTP 201, got {status} with {fresh:?}"
    );
    let minted = |body: &Value| -> Vec<String> {
        body["successfulRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|outcome| {
                outcome["memoryRecordId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };
    let first_ids = minted(&first);
    let fresh_ids = minted(&fresh);
    assert!(
        fresh_ids.iter().all(|id| !first_ids.contains(id)),
        "case \"a different token\": expected a second set of records, got {fresh_ids:?} against \
         {first_ids:?}"
    );

    // Both sets are readable, so the replay stored nothing extra and the fresh call
    // stored its own records.
    for id in first_ids.iter().chain(fresh_ids.iter()) {
        let record = stored_record(&router, MEMORY, id).await;
        assert_eq!(
            record["memoryRecordId"], *id,
            "case \"both sets readable\": expected {id:?} to be stored, got {record:?}"
        );
    }
}
