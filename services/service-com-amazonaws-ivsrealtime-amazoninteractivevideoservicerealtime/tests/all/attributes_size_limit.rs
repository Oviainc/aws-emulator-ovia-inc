// SPDX-License-Identifier: CC0-1.0

//! The documented 1 KB `attributes` bound, driven through EVERY operation of this service
//! that takes an `attributes` map: `CreateParticipantToken`, `CreateStage` (inside a
//! `participantTokenConfigurations` entry), `CreateIngestConfiguration` and
//! `StartParticipantReplication`.
//!
//! The rule under test: a map whose JSON encoding is larger than the documented maximum
//! is refused with the modelled `ValidationException` (HTTP 400), worded in the
//! member-constraint envelope this service states a rejection of one member's value with
//! — `Request member: <member> failed to satisfy the constraints: <member> is required to
//! have a maximum size of 1 KB, found <n> bytes` — and a map at or under it is accepted,
//! whatever shape the map has.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

use crate::attributes_measure::{json_size, oversized_message, raw_total, MAX_ATTRIBUTES_BYTES};

/// The capability that makes a participant a publisher, from
/// `smithy_query(query="shape",shape="ParticipantTokenCapability")` — the one
/// `StartParticipantReplication` requires of the participant it replicates.
const PUBLISH_CAPABILITY: &str = "PUBLISH";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in `us-east-1`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.us-east-1.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// A publishing participant of `stage_arn`, from a token issued for it.
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
        let request = json!({
            "stageArn": stage_arn,
            "capabilities": [PUBLISH_CAPABILITY],
        });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The shapes an `attributes` map of a given size can take. The rule bounds the size of
/// the map as JSON, so a map of one long value, a map of many short pairs and a map of
/// multi-byte text are the same input to it at the same size.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// One pair whose value carries almost all of the bytes.
    OneLongValue,
    /// The bytes spread over this many pairs.
    Pairs(usize),
    /// One pair whose value is two-byte characters, so its byte count is twice its
    /// character count.
    MultiByteValue,
}

/// How many entries a map of `shape` holds.
fn pairs_of(shape: Shape) -> usize {
    match shape {
        Shape::OneLongValue | Shape::MultiByteValue => 1,
        Shape::Pairs(pairs) => pairs,
    }
}

/// An `attributes` map of `shape` whose JSON encoding is exactly `total` bytes: the keys
/// and values carry `total` minus the punctuation `raw_total`
/// (`crate::attributes_measure`) accounts for, since nothing in these maps needs escaping.
fn attributes(shape: Shape, total: usize) -> BTreeMap<String, String> {
    let total = raw_total(pairs_of(shape), total);
    let mut map = BTreeMap::new();
    match shape {
        Shape::OneLongValue => {
            let key = "seat";
            map.insert(key.to_string(), "v".repeat(total - key.len()));
        }
        Shape::Pairs(pairs) => {
            // Each key is five bytes (`k0000`), so every pair needs at least six.
            assert!(
                total >= pairs * 6,
                "a {total}-byte map does not fit {pairs} pairs"
            );
            let per_pair = total / pairs;
            for index in 0..pairs {
                let key = format!("k{index:04}");
                // The last pair absorbs the division remainder, so the map totals exactly
                // `total` bytes.
                let budget = if index + 1 == pairs {
                    total - per_pair * (pairs - 1)
                } else {
                    per_pair
                };
                map.insert(key.clone(), "v".repeat(budget - key.len()));
            }
        }
        Shape::MultiByteValue => {
            // `é` is two bytes in UTF-8; an odd total takes one single-byte character.
            let key = "ключ";
            let value_bytes = total - key.len();
            let mut value = "é".repeat(value_bytes / 2);
            if value_bytes % 2 == 1 {
                value.push('v');
            }
            map.insert(key.to_string(), value);
        }
    }
    map
}

/// Assert one call's verdict against the rule: accepted under the bound, and refused past
/// it with the modelled error code and the stated sentence.
fn assert_verdict(case: &str, expected: &Expectation, status: u16, response: &Value) {
    match expected {
        Expectation::Accepted => assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {response}"
        ),
        Expectation::Refused(message) => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {response}"
            );
            assert_eq!(
                error_code(response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            let stated = member(response, "message");
            assert_eq!(
                &stated, message,
                "case {case:?}: expected the stated bound {message:?}, got {stated:?}"
            );
        }
    }
}

/// What the rule says about one map, on one operation and member path.
enum Expectation {
    Accepted,
    Refused(String),
}

/// The verdict the rule gives a map of `size` bytes sent in `member`.
fn expectation(member: &str, size: usize) -> Expectation {
    if size <= MAX_ATTRIBUTES_BYTES {
        Expectation::Accepted
    } else {
        Expectation::Refused(oversized_message(member, size))
    }
}

/// The totals every operation is driven over: each published bound, one byte either side
/// of the maximum, and totals well inside and well past it. The smallest is sized so
/// every shape below fits inside it once the JSON punctuation is paid for.
fn totals() -> Vec<usize> {
    vec![
        128,
        MAX_ATTRIBUTES_BYTES / 2,
        MAX_ATTRIBUTES_BYTES - 1,
        MAX_ATTRIBUTES_BYTES,
        MAX_ATTRIBUTES_BYTES + 1,
        MAX_ATTRIBUTES_BYTES * 4,
    ]
}

/// The shapes every operation is driven over, at every total.
fn shapes() -> Vec<Shape> {
    vec![
        Shape::OneLongValue,
        Shape::Pairs(2),
        Shape::Pairs(8),
        Shape::MultiByteValue,
    ]
}

/// `CreateParticipantToken` bounds its own `attributes` member, at every total and in
/// every shape.
#[tokio::test]
async fn create_participant_token_bounds_its_attributes_map() {
    let emulator = RealTime::new();
    let stage_arn = emulator.stage("the token's stage", "token-attributes").await;

    for shape in shapes() {
        for total in totals() {
            let map = attributes(shape, total);
            let size = json_size(&map);
            let case = format!("CreateParticipantToken, {shape:?} of {size} bytes");
            let (status, response) = emulator
                .call(
                    "CreateParticipantToken",
                    &json!({ "stageArn": stage_arn, "attributes": map }),
                )
                .await;
            let expected = expectation("attributes", size);
            assert_verdict(&case, &expected, status, &response);
            if let Expectation::Accepted = expected {
                // The accepted map is the one the minted token reports: the bound refuses,
                // it never trims.
                let reported: BTreeMap<String, String> =
                    serde_json::from_value(response["participantToken"]["attributes"].clone())
                        .unwrap_or_default();
                assert_eq!(
                    reported, map,
                    "case {case:?}: expected the attributes to be reported unchanged, \
                     got {reported:?}"
                );
            }
        }
    }
}

/// `CreateStage` bounds the `attributes` of EVERY `participantTokenConfigurations` entry,
/// and names the offending entry's position in the refusal.
#[tokio::test]
async fn create_stage_bounds_each_participant_token_configuration() {
    let emulator = RealTime::new();

    for shape in shapes() {
        for total in totals() {
            // Rows differ in the position the bounded map sits at, so the dotted member
            // path the refusal names is exercised beyond the first entry.
            for position in [1usize, 3] {
                let map = attributes(shape, total);
                let size = json_size(&map);
                let case =
                    format!("CreateStage, {shape:?} of {size} bytes at entry {position}");
                let mut configurations: Vec<Value> = Vec::new();
                for entry in 1..=position {
                    configurations.push(if entry == position {
                        json!({ "userId": format!("u{entry}"), "attributes": map })
                    } else {
                        json!({ "userId": format!("u{entry}") })
                    });
                }
                let (status, response) = emulator
                    .call(
                        "CreateStage",
                        &json!({ "participantTokenConfigurations": configurations }),
                    )
                    .await;
                let expected = expectation(
                    &format!("participantTokenConfigurations.{position}.member.attributes"),
                    size,
                );
                assert_verdict(&case, &expected, status, &response);
                if let Expectation::Accepted = expected {
                    let tokens = response["participantTokens"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let reported: BTreeMap<String, String> = tokens
                        .get(position - 1)
                        .map(|token| {
                            serde_json::from_value(token["attributes"].clone())
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    assert_eq!(
                        reported, map,
                        "case {case:?}: expected the entry's attributes on its token, \
                         got {reported:?}"
                    );
                }
            }
        }
    }
}

/// `CreateIngestConfiguration` bounds its own `attributes` member, at every total and in
/// every shape.
#[tokio::test]
async fn create_ingest_configuration_bounds_its_attributes_map() {
    let emulator = RealTime::new();

    for shape in shapes() {
        for total in totals() {
            let map = attributes(shape, total);
            let size = json_size(&map);
            let case = format!("CreateIngestConfiguration, {shape:?} of {size} bytes");
            let (status, response) = emulator
                .call(
                    "CreateIngestConfiguration",
                    &json!({ "ingestProtocol": "RTMPS", "attributes": map }),
                )
                .await;
            let expected = expectation("attributes", size);
            assert_verdict(&case, &expected, status, &response);
            if let Expectation::Accepted = expected {
                let reported: BTreeMap<String, String> = serde_json::from_value(
                    response["ingestConfiguration"]["attributes"].clone(),
                )
                .unwrap_or_default();
                assert_eq!(
                    reported, map,
                    "case {case:?}: expected the attributes to be reported unchanged, \
                     got {reported:?}"
                );
            }
        }
    }
}

/// `StartParticipantReplication` bounds its own `attributes` member, at every total and in
/// every shape.
#[tokio::test]
async fn start_participant_replication_bounds_its_attributes_map() {
    let emulator = RealTime::new();
    let source = emulator.stage("the source stage", "replication-source").await;
    let destination = emulator
        .stage("the destination stage", "replication-destination")
        .await;

    for shape in shapes() {
        for total in totals() {
            let map = attributes(shape, total);
            let size = json_size(&map);
            let case = format!("StartParticipantReplication, {shape:?} of {size} bytes");
            // A fresh publisher per row: a replication this store already holds is a
            // conflict, which would answer about something other than the bound.
            let publisher = emulator.publisher(&case, &source).await;
            let (status, response) = emulator
                .call(
                    "StartParticipantReplication",
                    &json!({
                        "sourceStageArn": source,
                        "destinationStageArn": destination,
                        "participantId": publisher,
                        "attributes": map,
                    }),
                )
                .await;
            let expected = expectation("attributes", size);
            assert_verdict(&case, &expected, status, &response);
        }
    }
}

/// A map the bound refuses leaves nothing behind: the same operation, called again with a
/// map under the bound, is accepted, so the refusal neither persisted a record nor
/// consumed anything.
#[tokio::test]
async fn a_refused_map_persists_nothing() {
    let emulator = RealTime::new();
    let stage_arn = emulator.stage("the token's stage", "refusal-leaves-none").await;
    let oversized = attributes(Shape::OneLongValue, MAX_ATTRIBUTES_BYTES + 1);
    let accepted = attributes(Shape::Pairs(4), MAX_ATTRIBUTES_BYTES);

    let probes: Vec<(&str, Value, Value)> = vec![
        (
            "CreateParticipantToken",
            json!({ "stageArn": stage_arn, "attributes": oversized }),
            json!({ "stageArn": stage_arn, "attributes": accepted }),
        ),
        (
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMPS", "attributes": oversized }),
            json!({ "ingestProtocol": "RTMPS", "attributes": accepted }),
        ),
        (
            "CreateStage",
            json!({ "participantTokenConfigurations": [{ "attributes": oversized }] }),
            json!({ "participantTokenConfigurations": [{ "attributes": accepted }] }),
        ),
    ];

    for (operation, refused_request, accepted_request) in probes {
        let case = format!("{operation} after a refusal");
        let (status, response) = emulator.call(operation, &refused_request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the oversized map to be refused, got {status} {response}"
        );
        let (status, response) = emulator.call(operation, &accepted_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the next call to be accepted, got {status} {response}"
        );
    }
}
