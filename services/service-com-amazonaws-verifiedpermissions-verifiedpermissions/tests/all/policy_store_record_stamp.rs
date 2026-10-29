// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the policy store record's own `lastUpdatedDate`.
//!
//! The rule under test: a request that is ACCEPTED and writes a member of the
//! policy-store record itself — its tag set, its settings or its schema — moves
//! that record's `lastUpdatedDate` to the instant of the call and leaves
//! `createdDate` where it was, so every later reader of the store sees the newer
//! stamp. A request that only READS the record, and one that is REFUSED before
//! anything is persisted, leaves the stamp exactly where it was.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, over ONE shared engine, reusing the harness in
//! `harness.rs` — so a stamp is only visible to a later request if it really
//! reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, ok, router};

/// The per-resource tag limit this service publishes, from `TagResource`'s own
/// modelled documentation ("You can associate as many as 50 tags with a
/// resource") and `TooManyTagsException`'s ("the limit (50)"). Written out here
/// rather than imported, so this suite pins the published quota.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One accepted write to the store's own record: the operation and its request.
struct Write {
    case: &'static str,
    operation: &'static str,
    request: Value,
}

/// One request that must leave the record's stamp alone, and the status it
/// answers with.
struct Untouched {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    error_code: Option<&'static str>,
}

/// The instant a `@timestampFormat("date-time")` member carries, in whole
/// nanoseconds since the epoch — compared as an integer so two stamps from the
/// same second are still ordered exactly. Anything that is not an RFC 3339 UTC
/// string is a failure.
fn stamp(case: &str, member: &str, body: &Value) -> i128 {
    let text = body[member].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be a date-time string, got {body}")
    });
    assert!(
        text.ends_with('Z'),
        "case {case:?}: {member} must be a UTC date-time, got {text:?}"
    );
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("case {case:?}: {member} {text:?} is not RFC 3339: {error}"));
    i128::from(parsed.secs()) * 1_000_000_000 + i128::from(parsed.subsec_nanos())
}

/// Create a store, with create-time tags, and return `(policyStoreId, arn)`.
async fn store(router: &MultiServiceRouter, tags: Value) -> (String, String) {
    let created = ok(
        router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "OFF" }, "tags": tags }),
    )
    .await;
    (
        created["policyStoreId"]
            .as_str()
            .expect("policyStoreId is required on the CreatePolicyStore response")
            .to_string(),
        created["arn"]
            .as_str()
            .expect("arn is required on the CreatePolicyStore response")
            .to_string(),
    )
}

/// Read the store back.
async fn get(router: &MultiServiceRouter, store_id: &str) -> Value {
    ok(
        router,
        "GetPolicyStore",
        &json!({ "policyStoreId": store_id, "tags": true }),
    )
    .await
}

/// A tag map of `count` distinct keys.
fn tag_map(prefix: &str, count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("{prefix}{index:03}"), json!("v")))
        .collect();
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// What moves the stamp
// ---------------------------------------------------------------------------

/// Every accepted write to a member of the store's own record moves its
/// `lastUpdatedDate`, whichever operation made the write and whatever it wrote.
#[tokio::test]
async fn every_accepted_write_to_the_store_record_moves_its_last_updated_date() {
    let router = router();
    let (store_id, arn) = store(&router, json!({ "team": "photos" })).await;

    // Rows differ along WHICH member of the record the write touches (tags,
    // settings, schema), which operation writes it, and how much it changes —
    // including writes whose net effect on the stored value is nothing.
    let writes = vec![
        Write {
            case: "a tag key appended",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "env": "test" } }),
        },
        Write {
            case: "several tag keys at once",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "owner": "alice", "tier": "gold" } }),
        },
        Write {
            case: "an existing tag value replaced",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "env": "prod" } }),
        },
        Write {
            case: "a tag pair already stored",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "env": "prod" } }),
        },
        Write {
            case: "a tag with an empty value",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "owner": "" } }),
        },
        Write {
            case: "one tag key removed",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["env"] }),
        },
        Write {
            case: "several tag keys removed, one of them absent",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["owner", "tier", "nosuchkey"] }),
        },
        Write {
            case: "only an absent tag key named",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["nosuchkey"] }),
        },
        Write {
            case: "the last tag key removed",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["team"] }),
        },
        Write {
            case: "the store's settings",
            operation: "UpdatePolicyStore",
            request: json!({
                "policyStoreId": store_id,
                "validationSettings": { "mode": "OFF" },
                "description": "demo",
            }),
        },
        Write {
            case: "the store's schema",
            operation: "PutSchema",
            request: json!({
                "policyStoreId": store_id,
                "definition": { "cedarJson": "{}" },
            }),
        },
        Write {
            case: "a tag key appended after the schema",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "stage": "beta" } }),
        },
    ];

    let before_all = get(&router, &store_id).await;
    let created = stamp("the created store", "createdDate", &before_all);
    let mut previous = stamp("the created store", "lastUpdatedDate", &before_all);
    assert_eq!(
        created, previous,
        "case \"the created store\": a new store's two stamps are the same instant, got {before_all}"
    );

    for Write {
        case,
        operation,
        request,
    } in &writes
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} {body}"
        );

        let fetched = get(&router, &store_id).await;
        let moved = stamp(case, "lastUpdatedDate", &fetched);
        assert!(
            moved > previous,
            "case {case:?}: {operation} must move the store's lastUpdatedDate past {previous}, \
             got {moved}"
        );
        assert_eq!(
            stamp(case, "createdDate", &fetched),
            created,
            "case {case:?}: createdDate must not move, got {fetched}"
        );
        previous = moved;
    }
}

// ---------------------------------------------------------------------------
// What leaves the stamp alone
// ---------------------------------------------------------------------------

/// A read of the record, and a write refused before anything is persisted,
/// leave `lastUpdatedDate` exactly where the last accepted write left it.
#[tokio::test]
async fn a_read_or_a_refused_write_leaves_the_store_record_stamp_where_it_was() {
    let router = router();
    let (store_id, arn) = store(&router, json!({})).await;

    // Fill the resource to the published tag limit, so the next new key is the
    // smallest request `TooManyTagsException` refuses.
    ok(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tag_map("key", MAX_TAGS_PER_RESOURCE) }),
    )
    .await;
    let settled = stamp(
        "the filled store",
        "lastUpdatedDate",
        &get(&router, &store_id).await,
    );

    // Rows differ along WHY the request writes nothing: it only reports, or it
    // is refused — on the store's tags and on its schema alike.
    let attempts = &[
        Untouched {
            case: "GetPolicyStore reports",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": store_id, "tags": true }),
            status: 200,
            error_code: None,
        },
        Untouched {
            case: "ListTagsForResource reports",
            operation: "ListTagsForResource",
            request: json!({ "resourceArn": arn }),
            status: 200,
            error_code: None,
        },
        Untouched {
            case: "GetSchema reports",
            operation: "GetSchema",
            request: json!({ "policyStoreId": store_id }),
            status: 200,
            error_code: None,
        },
        Untouched {
            case: "one tag key past the limit",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": { "onemore": "v" } }),
            status: 400,
            error_code: Some("TooManyTagsException"),
        },
        Untouched {
            case: "a whole request past the limit",
            operation: "TagResource",
            request: json!({
                "resourceArn": arn,
                "tags": tag_map("over", MAX_TAGS_PER_RESOURCE + 1),
            }),
            status: 400,
            error_code: Some("TooManyTagsException"),
        },
    ];

    for Untouched {
        case,
        operation,
        request,
        status: expected_status,
        error_code: expected_code,
    } in attempts
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from {operation}, got {status} {body}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                error_code(&body),
                *expected_code,
                "case {case:?}: unexpected error code, got {body}"
            );
        }

        let fetched = get(&router, &store_id).await;
        let now_reported = stamp(case, "lastUpdatedDate", &fetched);
        assert_eq!(
            now_reported, settled,
            "case {case:?}: the store's lastUpdatedDate must stay at {settled}, got {now_reported}"
        );
    }
}
