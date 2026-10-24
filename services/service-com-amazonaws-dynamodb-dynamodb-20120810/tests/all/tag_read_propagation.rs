// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the VISIBILITY of a DynamoDB tag change: when a tag write
//! reaches `ListTagsOfResource`, driven over the wire (`POST /` with `X-Amz-Target:
//! DynamoDB_20120810.<OperationName>`) through the same `MultiServiceRouter` the
//! emulator's front doors use.
//!
//! The rule under test: a tag write applies to the resource at once but reaches the
//! EVENTUALLY CONSISTENT tag reader only after a propagation span, so a
//! `ListTagsOfResource` taken inside that span reports the set that was visible before
//! the write and one taken after it reports the written set. "TagResource is an
//! asynchronous operation. If you issue a ListTagsOfResource request immediately after a
//! TagResource request, DynamoDB might return your previous tag set, if there was one,
//! or an empty tag set. This is because ListTagsOfResource uses an eventually consistent
//! query, and the metadata for your tags or table might not be available at that moment.
//! Wait for a few seconds, and then try the ListTagsOfResource request again. The
//! application or removal of tags using TagResource and UntagResource APIs is eventually
//! consistent. ListTagsOfResource API will only reflect the changes after a few seconds"
//! (the `TagResource` doc, repeated on `UntagResource`). AWS publishes no figure for "a
//! few seconds", so the span is the emulator's own
//! [`service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION`], which
//! these tests read rather than hard-code.
//!
//! The rows below span what the rule generalises over — the KIND of change (the set a
//! create seeds, a first tag, a batch, an overwrite, one key removed, every key removed,
//! a removal that changes nothing), the number of tags and the value — not one recorded
//! example. `resource_tags` covers WHAT each of those calls leaves stored; this suite
//! covers WHEN it is reported.

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

/// The region these tests sign for.
const REGION: &str = "us-east-1";

/// Long enough for a propagation the clock owes to have fallen due.
fn past_the_span() -> Duration {
    STATUS_TRANSITION + Duration::from_millis(50)
}

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

/// Send one operation and return its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", authorization(REGION))
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
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?} on {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// `(key, value)` pairs as the modelled `TagList` carries them.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The expected tag map, as [`listed_tags`] reports one.
fn expected_map(tags: &[(&str, &str)]) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect()
}

/// What `ListTagsOfResource` reports for `arn` RIGHT NOW, as a key -> value map. No
/// wait: these tests are about the instant a read is taken.
async fn listed_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Map<String, Value> {
    let listed = ok(
        router,
        case,
        "ListTagsOfResource",
        json!({"ResourceArn": arn}),
    )
    .await;
    let mut set = Map::new();
    for tag in listed["Tags"].as_array().cloned().unwrap_or_default() {
        let key = tag["Key"].as_str().unwrap_or("<missing>").to_string();
        set.insert(key, tag["Value"].clone());
    }
    set
}

/// Create a table carrying `tags` and return the ARN the service minted for it. The
/// caller decides whether to wait: a create's own tag set propagates like any other
/// change.
async fn create_tagged_table(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &[(&str, &str)],
) -> String {
    let created = ok(
        router,
        case,
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
    created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The write whose visibility one row exercises.
enum Change {
    /// A `TagResource` carrying these tags.
    Tag(&'static [(&'static str, &'static str)]),
    /// An `UntagResource` naming these keys.
    Untag(&'static [&'static str]),
}

/// One propagation case: the set the create seeds, the change applied to it, and the set
/// that change leaves once it has propagated.
struct PropagationCase {
    name: &'static str,
    seeded: &'static [(&'static str, &'static str)],
    change: Change,
    settled: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_tag_change_is_reported_only_once_its_propagation_span_has_elapsed() {
    let router = emulator();

    let cases: [PropagationCase; 8] = [
        PropagationCase {
            name: "a first tag on an untagged resource",
            seeded: &[],
            change: Change::Tag(&[("owner", "platform")]),
            settled: &[("owner", "platform")],
        },
        PropagationCase {
            name: "a batch of three keys at once",
            seeded: &[],
            change: Change::Tag(&[("env", "probe"), ("project", "probe-set"), ("team", "core")]),
            settled: &[("env", "probe"), ("project", "probe-set"), ("team", "core")],
        },
        PropagationCase {
            name: "an empty value, which the model allows (TagValueString len 0..256)",
            seeded: &[],
            change: Change::Tag(&[("owner", "")]),
            settled: &[("owner", "")],
        },
        PropagationCase {
            name: "a new key merging into the set the create seeded",
            seeded: &[("env", "staging")],
            change: Change::Tag(&[("owner", "platform")]),
            settled: &[("env", "staging"), ("owner", "platform")],
        },
        PropagationCase {
            name: "an existing key overwritten",
            seeded: &[("env", "staging")],
            change: Change::Tag(&[("env", "prod")]),
            settled: &[("env", "prod")],
        },
        PropagationCase {
            name: "one key removed and the others left",
            seeded: &[("env", "prod"), ("owner", "platform")],
            change: Change::Untag(&["owner"]),
            settled: &[("env", "prod")],
        },
        PropagationCase {
            name: "every key removed",
            seeded: &[("env", "prod"), ("owner", "platform")],
            change: Change::Untag(&["env", "owner"]),
            settled: &[],
        },
        PropagationCase {
            name: "a removal naming a key the resource does not carry",
            seeded: &[("env", "prod")],
            change: Change::Untag(&["absent"]),
            settled: &[("env", "prod")],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let (name, seeded, settled) = (case.name, case.seeded, case.settled);
        let table = format!("propagation-{index}");
        let arn = create_tagged_table(&router, name, &table, seeded).await;

        // The set a CreateTable seeds propagates like any other tag change, so it is
        // not visible to the reader yet.
        let immediately = listed_tags(&router, name, &arn).await;
        assert!(
            immediately.is_empty(),
            "case {name:?}: a create's tag set has not propagated yet, so the read reports \
             none, got {immediately:?}"
        );

        // Once the span has elapsed the seeded set is what the reader reports.
        tokio::time::sleep(past_the_span()).await;
        let propagated = listed_tags(&router, name, &arn).await;
        assert_eq!(
            propagated,
            expected_map(seeded),
            "case {name:?}: the seeded set is visible once its span has elapsed, expected \
             {:?}, got {propagated:?}",
            expected_map(seeded)
        );

        // The change under test: accepted at once, with an empty body.
        let (operation, body) = match case.change {
            Change::Tag(tags) => (
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tag_list(tags)}),
            ),
            Change::Untag(keys) => (
                "UntagResource",
                json!({"ResourceArn": arn, "TagKeys": keys}),
            ),
        };
        let written = ok(&router, name, operation, body).await;
        assert!(
            written.is_null() || written == json!({}),
            "case {name:?} on {operation}: expected an empty 200 body, got {written}"
        );

        // Inside the span the reader still reports the set that was visible before the
        // write — and reads do not move it, so repeating the read answers the same.
        for attempt in 0..2 {
            let inside = listed_tags(&router, name, &arn).await;
            assert_eq!(
                inside,
                expected_map(seeded),
                "case {name:?}: read {attempt} inside the propagation span reports the \
                 previous set {:?}, got {inside:?}",
                expected_map(seeded)
            );
        }

        // Past the span the written set is what the reader reports.
        tokio::time::sleep(past_the_span()).await;
        let after = listed_tags(&router, name, &arn).await;
        assert_eq!(
            after,
            expected_map(settled),
            "case {name:?}: the change is visible once its span has elapsed, expected {:?}, \
             got {after:?}",
            expected_map(settled)
        );

        // And it stays there: nothing further is owed, so a later read answers the same.
        tokio::time::sleep(past_the_span()).await;
        let settled_again = listed_tags(&router, name, &arn).await;
        assert_eq!(
            settled_again, after,
            "case {name:?}: a settled tag set does not move again, got {settled_again:?}"
        );
    }
}

#[tokio::test]
async fn several_writes_inside_one_span_settle_on_the_last_one_written() {
    let router = emulator();
    let case = "three writes inside one propagation span";
    let arn = create_tagged_table(&router, case, "burst", &[("env", "staging")]).await;
    tokio::time::sleep(past_the_span()).await;

    // Three changes in quick succession: an add, an overwrite of the seeded key, and a
    // removal. Each lands on the resource as it arrives.
    for (operation, body) in [
        (
            "TagResource",
            json!({"ResourceArn": arn, "Tags": [{"Key": "owner", "Value": "platform"}]}),
        ),
        (
            "TagResource",
            json!({"ResourceArn": arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": ["owner"]}),
        ),
    ] {
        ok(&router, case, operation, body).await;
    }

    // None of them is visible yet: the reader still reports what the create seeded.
    let inside = listed_tags(&router, case, &arn).await;
    assert_eq!(
        inside,
        expected_map(&[("env", "staging")]),
        "case {case:?}: inside the span the reader reports the previously visible set, got \
         {inside:?}"
    );

    // Once the span has elapsed the reader reports the set the LAST write left, not an
    // intermediate one.
    tokio::time::sleep(past_the_span()).await;
    let after = listed_tags(&router, case, &arn).await;
    assert_eq!(
        after,
        expected_map(&[("env", "prod")]),
        "case {case:?}: the newest propagated set is the one reported, expected {:?}, got \
         {after:?}",
        expected_map(&[("env", "prod")])
    );
}

#[tokio::test]
async fn a_write_is_applied_to_the_stored_set_even_before_the_reader_can_see_it() {
    let router = emulator();
    let case = "the ceiling is measured against the stored set";

    // The documented maximum tag set ("You can add up to 50 tags to a single DynamoDB
    // table" — the `Tag` doc), seeded by the create and therefore not visible yet.
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_TABLE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let full_borrowed: Vec<(&str, &str)> = full
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let arn = create_tagged_table(&router, case, "stored-set", &full_borrowed).await;

    let invisible = listed_tags(&router, case, &arn).await;
    assert!(
        invisible.is_empty(),
        "case {case:?}: the seeded set has not propagated yet, got {invisible:?}"
    );

    // The WRITE path sees the stored set regardless: one more key would push the
    // resource past the ceiling and is refused, even though the reader reports none.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "one-too-many", "Value": "x"}]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LimitExceededException"),
        "case {case:?}: expected 400 LimitExceededException, got {status} with {body}"
    );

    // And what finally becomes visible is the stored set, with the refused key absent.
    tokio::time::sleep(past_the_span()).await;
    let visible = listed_tags(&router, case, &arn).await;
    assert_eq!(
        visible.len(),
        MAX_TAGS_PER_TABLE,
        "case {case:?}: the propagated set is the one the create stored, got {visible:?}"
    );
    assert!(
        !visible.contains_key("one-too-many"),
        "case {case:?}: a refused write is never propagated, got {visible:?}"
    );
}
