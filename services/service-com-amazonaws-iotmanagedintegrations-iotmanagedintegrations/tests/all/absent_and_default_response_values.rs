// SPDX-License-Identifier: CC0-1.0

//! How this service reports a value that is EMPTY and a mutation that has nothing left to
//! do, end to end over the wire.
//!
//! RULE 1: `GetCredentialLocker` (GET `/credential-lockers/{Identifier}`) projects the
//! STORED locker record onto its output shape, so a member the record always holds is
//! always reported and only a member the record holds no value for is left out: the tag map
//! is reported whatever it holds — the EMPTY map for a locker that carries no tag, however
//! it came to carry none — while the optional `Name` the create named none for stays
//! absent. `TagsMap`'s `@length` (1..50,
//! `smithy_query(query="shape",shape="TagsMap")`) bounds what one tag WRITE may carry, not
//! what a read reports.
//!
//! RULE 2: `DeleteConnectorDestination` (DELETE `/connector-destinations/{Identifier}`,
//! success 200, `@idempotent`) reports the state the caller asked for rather than whether
//! this call was the one that reached it: an `Identifier` the signing region holds a record
//! under is removed, and an `Identifier` it holds no record under — never created, already
//! deleted, or another region's — leaves every store untouched and answers the same
//! success, with no error member.
//!
//! The recorded units these were derived from
//! (`iot-managed-integrations-credential-locker-lifecycle#task#3` and
//! `iot-managed-integrations-connector-destination-error-paths#task#3`) are ONE row of the
//! tables below. The rows differ along what the rules generalise over: for rule 1, how many
//! pairs the tag map holds (none, one, several, the `@length` maximum), the form of a key
//! and of a value at their modelled bounds, and whether the map is empty because nothing
//! ever tagged the locker or because every key was removed again; for rule 2, the form of
//! the identifier inside `ConnectorDestinationId` (both `@length` bounds, the punctuation
//! the `@pattern` admits, digits only, mixed case) and the CAUSE of the absence (never
//! created, deleted, another region's).
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses and bodies asserted here are the ones a client sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::MAX_RESOURCE_TAGS;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// Another region's endpoint, for the cases that address one region's resource through
/// another's.
const OTHER_REGION: &str = "eu-central-1";

/// The collection routes the suite drives
/// (`smithy_query(query="digest")`: POST/GET `/credential-lockers`, POST
/// `/connector-destinations`, POST `/cloud-connectors`).
const LOCKERS: &str = "/credential-lockers";
const DESTINATIONS: &str = "/connector-destinations";
const CONNECTORS: &str = "/cloud-connectors";

/// The `@length` maximum of `TagKey`, in characters
/// (`smithy_query(query="shape",shape="TagsMap")` -> key `TagKey` length 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The `@length` bounds of `TagValue`, in characters
/// (`smithy_query(query="shape",shape="TagsMap")` -> value `TagValue` length 0..256) — so
/// the EMPTY STRING is a value a key may carry.
const TAG_VALUE_BOUNDS: (usize, usize) = (0, 256);

/// The `@length` bounds of `ConnectorDestinationId`, in characters
/// (`smithy_query(query="operation",operation="DeleteConnectorDestination")` ->
/// `Identifier` length 1..64, pattern `^[A-Za-z0-9-_]+$`).
const DESTINATION_ID_BOUNDS: (usize, usize) = (1, 64);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// How many members the body carries, counting an empty or absent body as none.
    fn member_count(&self) -> usize {
        self.body.as_object().map_or(0, serde_json::Map::len)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// [`emulator_over`] over an engine the test does not keep a handle to.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path_and_query, body).await
}

/// `ResourceArn` is a NON-greedy `@httpLabel`, so the ARN fills ONE path segment: every
/// byte outside the unreserved set is percent-encoded, which is what an SDK puts on the
/// wire.
fn label(arn: &str) -> String {
    arn.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A tag map from pairs, as the tables below state them.
fn map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// The tag map as a request or response body carries it.
fn as_object(tags: &BTreeMap<String, String>) -> Value {
    json!(tags)
}

/// `CreateCredentialLocker` with `body`, answering the `Id` it minted.
async fn create_locker(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let created = call(router, "POST", LOCKERS, Some(body)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateCredentialLocker, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an Id member, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// `GetCredentialLocker` for `id`.
async fn get_locker(router: &MultiServiceRouter, id: &str) -> Reply {
    call(router, "GET", &format!("{LOCKERS}/{id}"), None).await
}

/// Assert a locker read reports exactly `expected` on `Tags`, as a member that is THERE
/// whatever the map holds.
fn assert_reports_tags(case: &str, read: &Reply, expected: &BTreeMap<String, String>) {
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected HTTP 200 from GetCredentialLocker, got {} with body {:?}",
        read.status, read.body
    );
    let reported = read.member("Tags").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the read to report a Tags member, got body {:?}",
            read.body
        )
    });
    assert_eq!(
        reported,
        &as_object(expected),
        "case {case:?}: expected the stored tag map {:?}, got {reported:?}",
        as_object(expected)
    );
    assert!(
        reported.is_object(),
        "case {case:?}: expected Tags to be a map, got {reported:?}"
    );
}

/// RULE 1: a locker's read reports its stored tag map whatever that map holds — the empty
/// map included — while an optional member the record holds no value for stays absent.
#[tokio::test]
async fn a_locker_read_reports_its_stored_tag_map_including_the_empty_one() {
    let router = emulator();

    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let longest_value = "v".repeat(TAG_VALUE_BOUNDS.1);
    // `TagsMap` is `@length` 1..50, so a create may carry at most this many pairs.
    let at_the_map_maximum: Vec<(String, String)> = (0..MAX_RESOURCE_TAGS)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();

    // The rows differ along how many pairs the map holds and the form of its keys and
    // values, including both modelled bounds of `TagValue` and the maximum of `TagKey`.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no tag at all", Vec::new()),
        (
            "one pair",
            vec![("env".to_string(), "test".to_string())],
        ),
        (
            "several pairs",
            vec![
                ("env".to_string(), "test".to_string()),
                ("owner".to_string(), "platform".to_string()),
                ("team".to_string(), "iot".to_string()),
            ],
        ),
        (
            "a value at the TagValue minimum",
            vec![("note".to_string(), "v".repeat(TAG_VALUE_BOUNDS.0))],
        ),
        (
            "a key at the TagKey maximum",
            vec![(longest_key.clone(), "at-the-key-bound".to_string())],
        ),
        (
            "a value at the TagValue maximum",
            vec![("long".to_string(), longest_value.clone())],
        ),
        ("pairs at the TagsMap maximum", at_the_map_maximum),
    ];

    for (case, pairs) in &rows {
        let expected = map(pairs);
        // `TagsMap` is `@length` 1..50, so a row with no pair carries NO `Tags` member
        // rather than an empty one: the empty map is what the record holds, not what the
        // request sent.
        let mut body = json!({});
        if !expected.is_empty() {
            body["Tags"] = as_object(&expected);
        }
        let id = create_locker(&router, case, body).await;

        let read = get_locker(&router, &id).await;
        assert_reports_tags(case, &read, &expected);
        assert_eq!(
            read.string("Id"),
            Some(id.as_str()),
            "case {case:?}: expected the read to report the stored Id, got {:?}",
            read.member("Id")
        );
        // The asymmetry the rule rests on: a member the record holds NO value for is left
        // out, while the tag map is always there.
        assert!(
            read.member("Name").is_none(),
            "case {case:?}: a locker the create named none for reports no Name, got {:?}",
            read.member("Name")
        );

        // A read is a read: the same answer comes back a second time.
        let reread = get_locker(&router, &id).await;
        assert_reports_tags(&format!("{case}, reread"), &reread, &expected);
    }
}

/// RULE 1: the empty map is reported however the locker came to carry no tag — including a
/// locker whose tags were all REMOVED again through the tagging surface, which is the one
/// cause a create can never produce.
#[tokio::test]
async fn a_locker_whose_tags_were_removed_reports_the_empty_map_again() {
    let router = emulator();

    let seeded = map(&[
        ("env".to_string(), "prod".to_string()),
        ("owner".to_string(), "platform".to_string()),
    ]);
    let id = create_locker(
        &router,
        "removed-to-empty",
        json!({ "Name": "Locker with tags", "Tags": as_object(&seeded) }),
    )
    .await;
    let read = get_locker(&router, &id).await;
    assert_reports_tags("created with tags", &read, &seeded);
    let arn = read
        .string("Arn")
        .expect("the read reports the locker ARN")
        .to_string();

    // One more pair written through the ARN-addressed tagging surface, then every key
    // removed again: each step is visible through the locker's own read.
    let added = map(&[("added".to_string(), "later".to_string())]);
    let tagged = call(
        &router,
        "POST",
        &format!("/tags/{}", label(&arn)),
        Some(json!({ "Tags": as_object(&added) })),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "case \"tag write\": expected HTTP 200 from TagResource, got {} with body {:?}",
        tagged.status, tagged.body
    );
    let mut all = seeded.clone();
    all.extend(added.clone());
    assert_reports_tags("after the tag write", &get_locker(&router, &id).await, &all);

    let keys: Vec<String> = all.keys().cloned().collect();
    let query: String = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    let untagged = call(
        &router,
        "DELETE",
        &format!("/tags/{}?{query}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        untagged.status, 200,
        "case \"tag removal\": expected HTTP 200 from UntagResource, got {} with body {:?}",
        untagged.status, untagged.body
    );
    assert_reports_tags(
        "after every key was removed",
        &get_locker(&router, &id).await,
        &BTreeMap::new(),
    );
}

/// The `CloudConnectorId` a connector destination has to name: a C2C connector created in
/// `region` through the wire.
async fn cloud_connector(router: &MultiServiceRouter, region: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        CONNECTORS,
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding a cloud connector in {region:?}: expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .expect("the connector create answers with an Id")
        .to_string()
}

/// `CreateConnectorDestination` in `region` against `connector`, answering the `Id` it
/// minted.
async fn create_destination(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    connector: &str,
) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        DESTINATIONS,
        Some(json!({ "CloudConnectorId": connector, "AuthConfig": {} })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateConnectorDestination, got {} with \
         body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an Id member, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// `DeleteConnectorDestination` for `identifier`, signed for `region`.
async fn delete_destination(router: &MultiServiceRouter, region: &str, identifier: &str) -> Reply {
    call_in(
        router,
        region,
        "DELETE",
        &format!("{DESTINATIONS}/{identifier}"),
        None,
    )
    .await
}

/// Assert one delete answered the modelled success, carrying no error and no member.
fn assert_deleted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from DeleteConnectorDestination, got {} with \
         body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, None,
        "case {case:?}: a success carries no error code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.member_count(),
        0,
        "case {case:?}: the modelled output declares no member, got body {:?}",
        reply.body
    );
}

/// RULE 2: a delete of an identifier the signing region holds no record under answers the
/// modelled success and changes nothing, whatever the form of the identifier and whatever
/// made it absent.
#[tokio::test]
async fn a_delete_of_an_identifier_this_region_holds_none_under_is_a_success() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let connector = cloud_connector(&router, REGION).await;

    // One record this region DOES hold, so none of the successes below is about an empty
    // store, and one record of ANOTHER region, which this region's delete must not touch.
    let kept = create_destination(&router, REGION, "kept", &connector).await;
    let other_connector = cloud_connector(&router, OTHER_REGION).await;
    let elsewhere =
        create_destination(&router, OTHER_REGION, "elsewhere", &other_connector).await;

    // One identifier that WAS held in this region and was deleted, under the id the
    // service minted rather than one this test chose.
    let deleted = create_destination(&router, REGION, "deleted", &connector).await;
    assert_deleted(
        "the call that removed it",
        &delete_destination(&router, REGION, &deleted).await,
    );

    // The rows differ along the form of the identifier inside `ConnectorDestinationId`
    // (length 1..64, `^[A-Za-z0-9-_]+$`) and along the CAUSE of the absence.
    let (min, max) = DESTINATION_ID_BOUNDS;
    let rows: Vec<(String, String)> = vec![
        ("never created".to_string(), "no-such-destination".to_string()),
        (
            format!("{min} character, the length minimum"),
            "x".to_string(),
        ),
        (
            format!("{max} characters, the length maximum"),
            "x".repeat(max),
        ),
        (
            "the punctuation the pattern admits".to_string(),
            "dest-with_punctuation-0".to_string(),
        ),
        ("digits only".to_string(), "0123456789".to_string()),
        ("mixed case".to_string(), "MiXeDcAsEiD".to_string()),
        ("created and already deleted".to_string(), deleted.clone()),
        ("another region's record".to_string(), elsewhere.clone()),
    ];

    for (case, identifier) in &rows {
        assert_deleted(case, &delete_destination(&router, REGION, identifier).await);
        // Repeating it is the same answer: the state asked for is the state reported.
        assert_deleted(
            &format!("{case}, repeated"),
            &delete_destination(&router, REGION, identifier).await,
        );

        // Nothing was removed: this region's record is still readable, and so is the
        // other region's through its own endpoint.
        let held = call(&router, "GET", &format!("{DESTINATIONS}/{kept}"), None).await;
        assert_eq!(
            held.status, 200,
            "case {case:?}: the record this region holds must survive, got {} with body {:?}",
            held.status, held.body
        );
        let there = call_in(
            &router,
            OTHER_REGION,
            "GET",
            &format!("{DESTINATIONS}/{elsewhere}"),
            None,
        )
        .await;
        assert_eq!(
            there.status, 200,
            "case {case:?}: another region's record must survive, got {} with body {:?}",
            there.status, there.body
        );
    }

    // The store itself: exactly the two records that were never deleted, in the order they
    // were inserted.
    let store = engine.state::<ServiceState>();
    let held: Vec<String> = store.lock().connector_destination_order.clone();
    assert_eq!(
        held,
        vec![kept.clone(), elsewhere.clone()],
        "a delete with nothing to remove must leave the store as it was, got {held:?}"
    );
}

/// RULE 2: a delete that DOES find a record removes it — so the read that reported it
/// answers the modelled 404 afterwards — and the delete that follows reports the same
/// success without anything left to remove.
#[tokio::test]
async fn a_delete_removes_the_record_and_the_next_one_reports_the_same_success() {
    let router = emulator();
    let connector = cloud_connector(&router, REGION).await;

    for case in ["first", "second", "third"] {
        let id = create_destination(&router, REGION, case, &connector).await;
        let read = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created destination must read back, got {} with body {:?}",
            read.status, read.body
        );

        assert_deleted(case, &delete_destination(&router, REGION, &id).await);
        let gone = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted destination must not read back, got {} with body {:?}",
            gone.status, gone.body
        );
        assert_deleted(
            &format!("{case}, deleted twice"),
            &delete_destination(&router, REGION, &id).await,
        );
    }
}
