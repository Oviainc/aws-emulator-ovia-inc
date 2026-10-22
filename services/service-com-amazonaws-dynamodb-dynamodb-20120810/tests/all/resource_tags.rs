// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DynamoDB tagging surface — `TagResource`,
//! `UntagResource` and `ListTagsOfResource` — driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.
//!
//! The rule under test: a tagging call addresses a resource by ARN; `TagResource`
//! MERGES the request's tags into the set stored on that resource, overwriting the
//! value of a key already present and leaving every other key alone ("Each DynamoDB
//! table can only have up to one tag with the same key. If you try to add an existing
//! tag (same key), the existing tag value will be updated to the new value" —
//! `Tag$Key`); `UntagResource` removes exactly the keys named; and
//! `ListTagsOfResource` reports the stored set and nothing else. Both writers answer
//! an empty 200. The documented ceiling is 50 tags per table ("You can add up to 50
//! tags to a single DynamoDB table" — the `Tag` doc), the `aws:` key prefix is one
//! "the user cannot assign" (same doc), and an ARN naming no table is
//! `ResourceNotFoundException` while a value that is not a table ARN at all is a
//! `ValidationException`.
//!
//! Every table below spans the class the rule generalises over — the number of tags,
//! the value (including the empty one), the key's case, the modelled length bounds,
//! which keys a later call names — rather than one recorded example.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_TAGS_PER_TABLE, STATUS_TRANSITION,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The refusal-code reader is the harness's, so there is one implementation of it.
use crate::emulator::error_code;

/// The region most of these tests sign for; the region-scoping test uses a second one.
const REGION: &str = "us-east-1";

/// One emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4-shaped `Authorization` header for `region`: its credential scope is what
/// routes the request to this service (signing name `dynamodb`) and names the region
/// the service answers for. Request authentication is off in these tests, so the
/// signature itself is never checked.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

/// Send one operation signed for `region` and return its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", authorization(region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed.
async fn ok(router: &MultiServiceRouter, region: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, region, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {region} {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Create a table carrying `tags`, wait out the `CREATING` -> `ACTIVE` transition the
/// clock owes it, and return the ARN the service minted for it.
async fn create_tagged_table(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: &[(&str, &str)],
) -> String {
    let created = ok(
        router,
        region,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "Tags": tag_list(tags),
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.ends_with(&format!(":table/{name}")),
        "case create {name}: the table reports its own ARN, got {created}"
    );
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(50));
    arn
}

/// `(key, value)` pairs as the modelled `TagList` carries them.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The tag set `ListTagsOfResource` reports for `arn`, as a key -> value map, plus the
/// assertion that the operation mints no pagination token (a resource's whole set fits
/// in one page).
async fn listed_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Map<String, Value> {
    let listed = ok(
        router,
        region,
        "ListTagsOfResource",
        json!({"ResourceArn": arn}),
    )
    .await;
    assert!(
        listed.get("NextToken").is_none_or(Value::is_null),
        "case list {arn}: a whole tag set fits in one page, so no NextToken is issued, \
         got {listed}"
    );
    let mut set = Map::new();
    for tag in listed["Tags"].as_array().cloned().unwrap_or_default() {
        let key = tag["Key"].as_str().unwrap_or("<missing>").to_string();
        let replaced = set.insert(key.clone(), tag["Value"].clone());
        assert!(
            replaced.is_none(),
            "case list {arn}: a resource carries at most one tag per key, got {key:?} twice \
             in {listed}"
        );
    }
    set
}

/// The expected tag map, as `listed_tags` reports one.
fn expected_map(tags: &[(&str, &str)]) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect()
}

/// Assert a writer answered an empty 200 body: `TagResource` and `UntagResource` both
/// model `Unit` output, so there is nothing in the body to read.
fn assert_empty_body(case: &str, body: &Value) {
    assert!(
        body.is_null() || body == &json!({}),
        "case {case:?}: expected an empty 200 body, got {body}"
    );
}

/// One merge case: the tags the table is created with, the batch a later `TagResource`
/// applies, and the whole set the resource must then report. The rows differ along what
/// the rule generalises over — the number of tags, the value (including the empty one
/// and the modelled maximum length), the key's case and length, and whether the batch
/// overwrites a stored key, adds a new one or repeats a key inside itself.
struct MergeCase {
    name: &'static str,
    created: &'static [(&'static str, &'static str)],
    batch: &'static [(&'static str, &'static str)],
    expected: &'static [(&'static str, &'static str)],
}

/// A key at the modelled maximum length (`TagKeyString`, len 1..128).
fn longest_key() -> String {
    "k".repeat(128)
}

/// A value at the modelled maximum length (`TagValueString`, len 0..256).
fn longest_value() -> String {
    "v".repeat(256)
}

#[tokio::test]
async fn tagging_merges_the_batch_into_the_stored_set_and_reports_it_back() {
    let router = emulator();

    let cases: [MergeCase; 8] = [
        MergeCase {
            name: "a first tag on an untagged table",
            created: &[],
            batch: &[("owner", "platform")],
            expected: &[("owner", "platform")],
        },
        MergeCase {
            name: "a batch of several keys at once",
            created: &[],
            batch: &[("owner", "platform"), ("env", "prod"), ("cost-centre", "42")],
            expected: &[("cost-centre", "42"), ("env", "prod"), ("owner", "platform")],
        },
        MergeCase {
            name: "an empty value, which the model allows (len 0..256)",
            created: &[],
            batch: &[("owner", "")],
            expected: &[("owner", "")],
        },
        MergeCase {
            name: "a new key merges into the set the create seeded",
            created: &[("env", "staging")],
            batch: &[("owner", "platform")],
            expected: &[("env", "staging"), ("owner", "platform")],
        },
        MergeCase {
            name: "an existing key is overwritten, the others are untouched",
            created: &[("env", "staging"), ("owner", "platform")],
            batch: &[("env", "prod")],
            expected: &[("env", "prod"), ("owner", "platform")],
        },
        MergeCase {
            name: "tag keys are case sensitive, so Env and env are two tags",
            created: &[("env", "staging")],
            batch: &[("Env", "prod"), ("ENV", "dev")],
            expected: &[("ENV", "dev"), ("Env", "prod"), ("env", "staging")],
        },
        MergeCase {
            name: "an empty batch changes nothing",
            created: &[("env", "staging")],
            batch: &[],
            expected: &[("env", "staging")],
        },
        MergeCase {
            name: "a key repeated inside one batch keeps the last value",
            created: &[],
            batch: &[("env", "staging"), ("env", "prod")],
            expected: &[("env", "prod")],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let table = format!("merge-{index}");
        let arn = create_tagged_table(&router, REGION, &table, case.created).await;

        // The create's own tag set is already readable through the reader.
        let seeded = listed_tags(&router, REGION, &arn).await;
        assert_eq!(
            seeded,
            expected_map(case.created),
            "case {:?}: ListTagsOfResource reports the set CreateTable stored, got {seeded:?}",
            case.name
        );

        let tagged = ok(
            &router,
            REGION,
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tag_list(case.batch)}),
        )
        .await;
        assert_empty_body(case.name, &tagged);

        let listed = listed_tags(&router, REGION, &arn).await;
        assert_eq!(
            listed,
            expected_map(case.expected),
            "case {:?}: expected the merged set {:?}, got {listed:?}",
            case.name,
            expected_map(case.expected)
        );
    }

    // The modelled length bounds, at the maximum and one past it: a key of 128
    // characters and a value of 256 are stored, and a key of 129 is refused before the
    // handler runs.
    let arn = create_tagged_table(&router, REGION, "merge-bounds", &[]).await;
    let longest_key = longest_key();
    let longest_value = longest_value();
    let tagged = ok(
        &router,
        REGION,
        "TagResource",
        json!({
            "ResourceArn": arn,
            "Tags": [{"Key": longest_key, "Value": longest_value}],
        }),
    )
    .await;
    assert_empty_body("a key and value at the modelled maximum length", &tagged);
    let listed = listed_tags(&router, REGION, &arn).await;
    assert_eq!(
        listed.get(&longest_key).and_then(Value::as_str),
        Some(longest_value.as_str()),
        "case {:?}: the stored value is what was written, got {listed:?}",
        "a key and value at the modelled maximum length"
    );

    let (status, body) = call(
        &router,
        REGION,
        "TagResource",
        &json!({
            "ResourceArn": arn,
            "Tags": [{"Key": "k".repeat(129), "Value": "v"}],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected HTTP 400, got {status} with {body}",
        "a key one past the modelled maximum length"
    );
    let after = listed_tags(&router, REGION, &arn).await;
    assert_eq!(
        after.len(),
        1,
        "case {:?}: a refused write stores nothing, got {after:?}",
        "a key one past the modelled maximum length"
    );
}

/// One untag case: the keys `UntagResource` names, and the set that must remain.
struct UntagCase {
    name: &'static str,
    removed: &'static [&'static str],
    remaining: &'static [(&'static str, &'static str)],
}

/// One untag case: the keys `UntagResource` names, and the set that must remain. The
/// rows span the counts the rule generalises over — none, one, several, every key — and
/// keys the resource does not carry.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_named_and_is_idempotent() {
    let router = emulator();

    const STORED: &[(&str, &str)] = &[("env", "prod"), ("owner", "platform"), ("tier", "gold")];

    let cases: [UntagCase; 5] = [
        UntagCase {
            name: "no keys named leaves the set alone",
            removed: &[],
            remaining: STORED,
        },
        UntagCase {
            name: "one key goes and the others stay",
            removed: &["owner"],
            remaining: &[("env", "prod"), ("tier", "gold")],
        },
        UntagCase {
            name: "several keys at once",
            removed: &["env", "tier"],
            remaining: &[("owner", "platform")],
        },
        UntagCase {
            name: "every key leaves an empty set",
            removed: &["env", "owner", "tier"],
            remaining: &[],
        },
        UntagCase {
            name: "a key the resource does not carry removes nothing",
            removed: &["absent", "Env"],
            remaining: STORED,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let (case, keys, remaining) = (case.name, case.removed, case.remaining);
        let table = format!("untag-{index}");
        let arn = create_tagged_table(&router, REGION, &table, STORED).await;

        let untagged = ok(
            &router,
            REGION,
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": keys}),
        )
        .await;
        assert_empty_body(case, &untagged);

        let listed = listed_tags(&router, REGION, &arn).await;
        assert_eq!(
            listed,
            expected_map(remaining),
            "case {case:?}: expected {:?} to remain, got {listed:?}",
            expected_map(remaining)
        );

        // Idempotent: the same removal again changes nothing further.
        let repeated = ok(
            &router,
            REGION,
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": keys}),
        )
        .await;
        assert_empty_body(case, &repeated);
        let again = listed_tags(&router, REGION, &arn).await;
        assert_eq!(
            again, listed,
            "case {case:?}: repeating the removal changes nothing, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_documented_tag_ceiling_is_enforced_and_a_refused_write_stores_nothing() {
    let router = emulator();

    // Exactly the documented maximum is accepted at create time.
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_TABLE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let full_borrowed: Vec<(&str, &str)> = full
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let arn = create_tagged_table(&router, REGION, "ceiling", &full_borrowed).await;
    let listed = listed_tags(&router, REGION, &arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_TABLE,
        "case {:?}: a table may carry {MAX_TAGS_PER_TABLE} tags, got {listed:?}",
        "exactly the documented maximum"
    );

    // Overwriting a key the table already carries never grows the set, so it is still
    // accepted at the ceiling.
    let overwritten = ok(
        &router,
        REGION,
        "TagResource",
        json!({"ResourceArn": arn, "Tags": [{"Key": "key-00", "Value": "moved"}]}),
    )
    .await;
    assert_empty_body("overwriting a stored key at the ceiling", &overwritten);
    let listed = listed_tags(&router, REGION, &arn).await;
    assert_eq!(
        (listed.len(), listed.get("key-00").and_then(Value::as_str)),
        (MAX_TAGS_PER_TABLE, Some("moved")),
        "case {:?}: the value moves and the set does not grow, got {listed:?}",
        "overwriting a stored key at the ceiling"
    );

    // One key past the ceiling is refused with the modelled limit error, and the stored
    // set is unchanged.
    let (status, body) = call(
        &router,
        REGION,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "one-too-many", "Value": "x"}]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LimitExceededException"),
        "case {:?}: expected 400 LimitExceededException, got {status} with {body}",
        "one tag past the ceiling"
    );
    let after = listed_tags(&router, REGION, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_TABLE,
        "case {:?}: a refused write stores nothing, got {after:?}",
        "one tag past the ceiling"
    );
    assert!(
        !after.contains_key("one-too-many"),
        "case {:?}: the refused key is not stored, got {after:?}",
        "one tag past the ceiling"
    );

    // A create carrying one tag more than a table may hold is refused the same way.
    let mut too_many = full_borrowed.clone();
    too_many.push(("one-too-many", "x"));
    let (status, body) = call(
        &router,
        REGION,
        "CreateTable",
        &json!({
            "TableName": "ceiling-create",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "Tags": tag_list(&too_many),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LimitExceededException"),
        "case {:?}: expected 400 LimitExceededException, got {status} with {body}",
        "a create one tag past the ceiling"
    );
    let (status, body) = call(
        &router,
        REGION,
        "DescribeTable",
        &json!({"TableName": "ceiling-create"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {:?}: the refused create stored no table, got {status} with {body}",
        "a create one tag past the ceiling"
    );
}

#[tokio::test]
async fn a_key_in_the_reserved_namespace_is_refused_and_a_merely_similar_one_is_not() {
    let router = emulator();
    let arn = create_tagged_table(&router, REGION, "reserved", &[]).await;

    // (case, key, refused)
    let cases: [(&str, &str, bool); 5] = [
        ("the reserved prefix", "aws:cloudformation:stack-name", true),
        ("the reserved prefix, upper case", "AWS:billing", true),
        ("the reserved prefix, mixed case", "Aws:owner", true),
        ("a key that merely starts with aws", "awsome", false),
        ("a key with the prefix elsewhere", "team:aws:owner", false),
    ];

    for (case, key, refused) in cases {
        let (status, body) = call(
            &router,
            REGION,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": [{"Key": key, "Value": "v"}]}),
        )
        .await;
        let listed = listed_tags(&router, REGION, &arn).await;
        if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException for {key:?}, got {status} \
                 with {body}"
            );
            assert!(
                !listed.contains_key(key),
                "case {case:?}: a refused key is not stored, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {key:?}, got {status} with {body}"
            );
            assert_eq!(
                listed.get(key).and_then(Value::as_str),
                Some("v"),
                "case {case:?}: {key:?} is stored, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_naming_no_table_is_refused_on_every_tagging_operation() {
    let router = emulator();
    // One real table, so the refusals below are about the ARN and nothing else.
    let real = create_tagged_table(&router, REGION, "tagged-table", &[("env", "prod")]).await;
    let account = real
        .split(':')
        .nth(4)
        .expect("the minted ARN carries an account")
        .to_string();

    // (case, ResourceArn, expected error code). A well-formed table ARN naming a table
    // that is not there is a missing resource; a value that is not a table ARN of this
    // service is a bad request.
    let cases: [(&str, String, &str); 8] = [
        (
            "a table that was never created",
            format!("arn:aws:dynamodb:{REGION}:{account}:table/absent"),
            "ResourceNotFoundException",
        ),
        (
            "a table that exists only in another region",
            format!("arn:aws:dynamodb:eu-west-1:{account}:table/tagged-table"),
            "ResourceNotFoundException",
        ),
        (
            "a bare table name rather than an ARN",
            "tagged-table".to_string(),
            "ValidationException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{account}:tagged-table"),
            "ValidationException",
        ),
        (
            "an index sub-resource ARN",
            format!("arn:aws:dynamodb:{REGION}:{account}:table/tagged-table/index/by-env"),
            "ValidationException",
        ),
        (
            "a stream sub-resource ARN",
            format!(
                "arn:aws:dynamodb:{REGION}:{account}:table/tagged-table/stream/2026-01-01T00:00:00.000"
            ),
            "ValidationException",
        ),
        (
            "an ARN with no region",
            format!("arn:aws:dynamodb::{account}:table/tagged-table"),
            "ValidationException",
        ),
        (
            "an ARN naming another resource type",
            format!("arn:aws:dynamodb:{REGION}:{account}:backup/tagged-table"),
            "ValidationException",
        ),
    ];

    for (case, resource_arn, expected) in &cases {
        for (operation, body) in [
            (
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [{"Key": "k", "Value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["k"]}),
            ),
            (
                "ListTagsOfResource",
                json!({"ResourceArn": resource_arn}),
            ),
        ] {
            let (status, answered) = call(&router, REGION, operation, &body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, *expected),
                "case {case:?} on {operation}: expected 400 {expected}, got {status} with \
                 {answered}"
            );
        }
    }

    // None of those refusals touched the table that does exist.
    let listed = listed_tags(&router, REGION, &real).await;
    assert_eq!(
        listed,
        expected_map(&[("env", "prod")]),
        "case {:?}: the stored set is untouched by the refusals, got {listed:?}",
        "the real table"
    );
}

#[tokio::test]
async fn each_regions_table_keeps_its_own_tag_set() {
    let router = emulator();
    // The same table NAME in two regions is two independent tables, so each ARN
    // addresses its own tag set.
    let east = create_tagged_table(&router, "us-east-1", "shared-name", &[("where", "east")]).await;
    let west = create_tagged_table(&router, "us-west-2", "shared-name", &[("where", "west")]).await;
    assert_ne!(
        east, west,
        "case {:?}: the two tables have distinct ARNs, got {east} and {west}",
        "one name in two regions"
    );

    ok(
        &router,
        "us-east-1",
        "TagResource",
        json!({"ResourceArn": east, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;

    let eastern = listed_tags(&router, "us-east-1", &east).await;
    assert_eq!(
        eastern,
        expected_map(&[("env", "prod"), ("where", "east")]),
        "case {:?}: the tagged table reports both tags, got {eastern:?}",
        "one name in two regions"
    );
    let western = listed_tags(&router, "us-west-2", &west).await;
    assert_eq!(
        western,
        expected_map(&[("where", "west")]),
        "case {:?}: the other region's table is untouched, got {western:?}",
        "one name in two regions"
    );
}

#[tokio::test]
async fn a_pagination_token_this_operation_never_issued_is_refused() {
    let router = emulator();
    let arn = create_tagged_table(&router, REGION, "token", &[("env", "prod")]).await;

    for token in ["not-a-token", "eyJhZnRlciI6ICJlbnYifQ=="] {
        let (status, body) = call(
            &router,
            REGION,
            "ListTagsOfResource",
            &json!({"ResourceArn": arn, "NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {token:?}: expected 400 ValidationException, got {status} with {body}"
        );
    }

    // An absent or empty token is the first (and only) page.
    for (case, body) in [
        ("an absent token", json!({"ResourceArn": arn})),
        ("an empty token", json!({"ResourceArn": arn, "NextToken": ""})),
    ] {
        let listed = ok(&router, REGION, "ListTagsOfResource", body).await;
        assert_eq!(
            listed["Tags"],
            json!([{"Key": "env", "Value": "prod"}]),
            "case {case:?}: the whole set is the one page, got {listed}"
        );
    }
}
