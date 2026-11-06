// SPDX-License-Identifier: CC0-1.0

//! The documented 1 KB `attributes` bound on the map this service COMPOSES: the
//! attributes a replica participant carries in the destination stage.
//!
//! `StartParticipantReplication` does not store the map its request carries; it stores the
//! merge of that map over the attributes the replicated participant's own identity holds
//! ("These attributes are merged with any attributes set for this participant when
//! creating the token. If there is overlap in keys, the values in these attributes are
//! replaced" —
//! `smithy_query(query="doc",shape="StartParticipantReplicationRequest",member="attributes")`).
//! That merge is what `GetParticipant` reports as `Participant$attributes`, a field
//! documenting the very same maximum
//! (`smithy_query(query="doc",shape="Participant",member="attributes")`).
//!
//! The rule under test: the bound is measured on the map that will be STORED and REPORTED,
//! so a composition past the documented maximum is refused with the modelled
//! `ValidationException` (HTTP 400) in this service's constraint-refusal construction —
//! naming the member and the COMPOSED size — and a composition at or under
//! it is accepted and reported back unchanged. The rows span what the rule generalises
//! over: which half carries the bytes, whether the keys overlap (an overlapping key takes
//! the overriding value, so a composition can total less than the sum of its halves), how
//! many pairs the halves hold, single- against multi-byte text, and each side of the
//! maximum.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The documented maximum total size of an `attributes` map. The model states it on
/// `StartParticipantReplicationRequest$attributes` and on the field that reports the
/// composed map, `Participant$attributes`: "The maximum length of this field is 1 KB
/// total."
const MAX_ATTRIBUTES_BYTES: usize = 1024;

/// The operation whose composition this suite bounds.
const OPERATION: &str = "StartParticipantReplication";

/// The capability that makes a participant a publisher, from
/// `smithy_query(query="shape",shape="ParticipantTokenCapability")` — the one
/// `StartParticipantReplication` requires of the participant it replicates.
const PUBLISH_CAPABILITY: &str = "PUBLISH";

/// What `activeSessionId` reports for a stage no session is running in: the empty string.
const NO_ACTIVE_SESSION: &str = "";

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

    /// A publishing participant of `stage_arn` whose token carries `attributes`.
    async fn publisher(
        &self,
        case: &str,
        stage_arn: &str,
        attributes: &BTreeMap<String, String>,
    ) -> String {
        let request = json!({
            "stageArn": stage_arn,
            "capabilities": [PUBLISH_CAPABILITY],
            "attributes": attributes,
        });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// The `Participant` `GetParticipant` reports for one participant of one session.
    async fn participant(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let request = json!({
            "stageArn": stage_arn,
            "sessionId": session_id,
            "participantId": participant_id,
        });
        let (status, response) = self.call("GetParticipant", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participant"].clone()
    }

    /// The replicas this store holds for `participant_id` of `source_stage_arn`.
    async fn replicas(&self, case: &str, source_stage_arn: &str, participant_id: &str) -> usize {
        let request = json!({
            "sourceStageArn": source_stage_arn,
            "participantId": participant_id,
        });
        let (status, response) = self.call("ListParticipantReplicas", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantReplicas {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["replicas"].as_array().map_or(0, Vec::len)
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

/// The `attributes` a reported participant carries: the member is omitted for a
/// participant with none, which is the empty map.
fn reported_attributes(participant: &Value) -> BTreeMap<String, String> {
    participant
        .get("attributes")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// An `attributes` map of `pairs` pairs whose keys and values total exactly `total`
/// bytes, every key prefixed with `prefix` so two maps built with different prefixes
/// share no key.
fn attributes(prefix: &str, pairs: usize, total: usize) -> BTreeMap<String, String> {
    // Each key is the prefix plus a three-digit index, so every pair needs one byte more
    // than its key.
    let key_bytes = prefix.len() + 3;
    assert!(
        total >= pairs * (key_bytes + 1),
        "a {total}-byte map does not fit {pairs} pairs of {prefix:?}"
    );
    let per_pair = total / pairs;
    (0..pairs)
        .map(|index| {
            let key = format!("{prefix}{index:03}");
            // The last pair absorbs the division remainder, so the map totals exactly
            // `total` bytes.
            let budget = if index + 1 == pairs {
                total - per_pair * (pairs - 1)
            } else {
                per_pair
            };
            (key.clone(), "v".repeat(budget - key.len()))
        })
        .collect()
}

/// An `attributes` map of one pair whose value is two-byte UTF-8 text, totalling exactly
/// `total` bytes: the measure is BYTES, so this map is the same size as a single-byte one
/// of half the characters.
fn multi_byte_attributes(key: &str, total: usize) -> BTreeMap<String, String> {
    let value_bytes = total - key.len();
    let mut value = "é".repeat(value_bytes / 2);
    if value_bytes % 2 == 1 {
        value.push('v');
    }
    BTreeMap::from([(key.to_string(), value)])
}

/// One pair, so a row can state an overlapping key directly.
fn pair(key: &str, value_bytes: usize) -> BTreeMap<String, String> {
    BTreeMap::from([(key.to_string(), "v".repeat(value_bytes))])
}

/// The size the rule measures: the bytes the map occupies as JSON — the braces, the
/// quoted keys, the colons, the commas and the quoted values — which is the form every
/// request carries one in.
fn size_of(map: &BTreeMap<String, String>) -> usize {
    serde_json::to_string(map)
        .expect("a map of strings serializes")
        .len()
}

/// The JSON punctuation a map of `pairs` entries carries, which the measured size counts
/// beside the keys and values themselves: the two braces, the two quotes around each key
/// and each value, the colon in every entry and the comma between entries.
fn json_punctuation(pairs: usize) -> usize {
    2 + pairs * 5 + pairs.saturating_sub(1)
}

/// The keys-and-values total a map of `pairs` entries needs for its JSON encoding to be
/// exactly `json_total` bytes — how a row below lands a COMPOSITION of several entries on
/// the bound.
fn raw_total(pairs: usize, json_total: usize) -> usize {
    json_total - json_punctuation(pairs)
}

/// The map `StartParticipantReplication` stores: the participant's own attributes with the
/// request's merged over them, an overlapping key taking the request's value.
fn composed(
    token: &BTreeMap<String, String>,
    requested: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = token.clone();
    merged.extend(requested.iter().map(|(k, v)| (k.clone(), v.clone())));
    merged
}

/// The sentence this service states the bound with, for the composed size.
fn expected_message(size: usize) -> String {
    format!(
        "Request member: attributes failed to satisfy the constraints: attributes is \
         required to have a maximum size of 1 KB, found {size} bytes"
    )
}

/// One row of the class: the attributes the publisher's token carries and the attributes
/// the replication request carries.
struct Row {
    case: &'static str,
    token: BTreeMap<String, String>,
    /// `None` is a request that leaves the member off entirely.
    requested: Option<BTreeMap<String, String>>,
}

/// The rows the rule is driven over: each differs from the others in which half carries
/// the bytes, whether the halves share keys, how many pairs they hold, whether the text is
/// single- or multi-byte, and where the composed total sits against the maximum.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no attributes on either half",
            token: BTreeMap::new(),
            requested: None,
        },
        Row {
            case: "the request carries every byte, well under the maximum",
            token: BTreeMap::new(),
            requested: Some(attributes("r", 2, 600)),
        },
        Row {
            case: "the token carries every byte, well under the maximum",
            token: attributes("t", 3, 600),
            requested: None,
        },
        Row {
            case: "an empty map on the request leaves the token's attributes composed",
            token: attributes("t", 1, 300),
            requested: Some(BTreeMap::new()),
        },
        Row {
            case: "two halves each legal on their own that compose past the maximum",
            token: attributes("t", 2, 600),
            requested: Some(attributes("r", 2, 600)),
        },
        Row {
            case: "disjoint halves composing to exactly the maximum",
            token: attributes("t", 4, raw_total(8, MAX_ATTRIBUTES_BYTES) / 2),
            requested: Some(attributes(
                "r",
                4,
                raw_total(8, MAX_ATTRIBUTES_BYTES) - raw_total(8, MAX_ATTRIBUTES_BYTES) / 2,
            )),
        },
        Row {
            case: "disjoint halves composing to one byte past the maximum",
            token: attributes("t", 4, raw_total(8, MAX_ATTRIBUTES_BYTES) / 2),
            requested: Some(attributes(
                "r",
                4,
                raw_total(8, MAX_ATTRIBUTES_BYTES) - raw_total(8, MAX_ATTRIBUTES_BYTES) / 2 + 1,
            )),
        },
        Row {
            case: "many small pairs on both halves, composing past the maximum",
            token: attributes("t", 16, 528),
            requested: Some(attributes("r", 16, 528)),
        },
        Row {
            case: "the token at the maximum and the request adding one byte on a new key",
            token: attributes("t", 1, raw_total(1, MAX_ATTRIBUTES_BYTES)),
            requested: Some(pair("r", 0)),
        },
        Row {
            case: "the token at the maximum with no request attributes",
            token: attributes("t", 2, raw_total(2, MAX_ATTRIBUTES_BYTES)),
            requested: None,
        },
        Row {
            case: "an overlapping key replacing a large value with a small one",
            token: pair("seat", raw_total(1, MAX_ATTRIBUTES_BYTES) - "seat".len()),
            requested: Some(pair("seat", 8)),
        },
        Row {
            case: "an overlapping key replacing a small value with one past the maximum",
            token: pair("seat", 8),
            requested: Some(pair(
                "seat",
                raw_total(1, MAX_ATTRIBUTES_BYTES) - "seat".len() + 1,
            )),
        },
        Row {
            case: "multi-byte text on both halves, composing past the maximum",
            token: multi_byte_attributes("ключ", 600),
            requested: Some(multi_byte_attributes("клавиша", 600)),
        },
        Row {
            case: "multi-byte text on both halves, composing to exactly the maximum",
            token: multi_byte_attributes("ключ", raw_total(2, MAX_ATTRIBUTES_BYTES) / 2),
            requested: Some(multi_byte_attributes(
                "клавиша",
                raw_total(2, MAX_ATTRIBUTES_BYTES) - raw_total(2, MAX_ATTRIBUTES_BYTES) / 2,
            )),
        },
        Row {
            case: "a request map one byte past the maximum on its own",
            token: BTreeMap::new(),
            requested: Some(attributes("r", 1, raw_total(1, MAX_ATTRIBUTES_BYTES) + 1)),
        },
    ]
}

/// The composed map is what the bound is measured against, and what the destination
/// stage's participant reports: a composition at or under the maximum is accepted and read
/// back unchanged, and one past it is refused with the modelled `ValidationException`
/// naming the composed total.
#[tokio::test]
async fn the_composed_replica_attributes_are_bounded_and_reported() {
    let emulator = RealTime::new();

    for row in rows() {
        let case = row.case;
        let requested = row.requested.clone().unwrap_or_default();
        let merged = composed(&row.token, &requested);
        let merged_size = size_of(&merged);

        // Fresh stages per row: a replication this store already holds is a conflict,
        // which would answer about something other than the bound.
        let source = emulator.stage(case, "merge-source").await;
        let destination = emulator.stage(case, "merge-destination").await;
        let publisher = emulator.publisher(case, &source, &row.token).await;

        let mut request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": publisher,
        });
        if let Some(map) = row.requested.as_ref() {
            request["attributes"] = json!(map);
        }
        let (status, response) = emulator.call(OPERATION, &request).await;

        if merged_size <= MAX_ATTRIBUTES_BYTES {
            assert_eq!(
                status, 200,
                "case {case:?}: a composition of {merged_size} bytes must answer HTTP 200, \
                 got {status} {response}"
            );

            // Round trip: the stored composition is what the replica participant of the
            // destination session reports.
            let session = emulator.active_session(case, &destination).await;
            assert_ne!(
                session, NO_ACTIVE_SESSION,
                "case {case:?}: the destination stage must have a session after the \
                 replication, got {session:?}"
            );
            let participant = emulator
                .participant(case, &destination, &session, &publisher)
                .await;
            let reported = reported_attributes(&participant);
            assert_eq!(
                reported, merged,
                "case {case:?}: expected the replica to report the composed attributes \
                 {merged:?}, got {reported:?}"
            );
            let reported_size = size_of(&reported);
            assert!(
                reported_size <= MAX_ATTRIBUTES_BYTES,
                "case {case:?}: expected the reported attributes within \
                 {MAX_ATTRIBUTES_BYTES} bytes, got {reported_size}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a composition of {merged_size} bytes must answer HTTP 400, \
                 got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            let stated = member(&response, "message");
            let expected = expected_message(merged_size);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the stated bound {expected:?}, got {stated:?}"
            );
        }
    }
}

/// A refused composition persists nothing: no replica is recorded, the destination stage
/// is left with no session, and the same replication with a composition under the bound is
/// then accepted.
#[tokio::test]
async fn a_refused_composition_leaves_the_store_as_it_was() {
    let emulator = RealTime::new();
    let case = "a refused composition";
    let source = emulator.stage(case, "refused-source").await;
    let destination = emulator.stage(case, "refused-destination").await;
    // The token holds the whole maximum, so any non-empty new key on the request composes
    // past it.
    let token = attributes("t", 2, raw_total(2, MAX_ATTRIBUTES_BYTES));
    let publisher = emulator.publisher(case, &source, &token).await;

    let refused = json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": publisher,
        "attributes": attributes("r", 1, 64),
    });
    let (status, response) = emulator.call(OPERATION, &refused).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the oversized composition must be refused, got {status} {response}"
    );

    let held = emulator.replicas(case, &source, &publisher).await;
    assert_eq!(
        held, 0,
        "case {case:?}: a refused start must record no replica, got {held}"
    );
    let session = emulator.active_session(case, &destination).await;
    assert_eq!(
        session, NO_ACTIVE_SESSION,
        "case {case:?}: a refused start must open no session in the destination stage, \
         got {session:?}"
    );

    // The same replication whose composition fits is accepted afterwards: the refusal
    // consumed nothing.
    let accepted = json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": publisher,
    });
    let (status, response) = emulator.call(OPERATION, &accepted).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the composition at the maximum must be accepted, \
         got {status} {response}"
    );
    let held = emulator.replicas(case, &source, &publisher).await;
    assert_eq!(
        held, 1,
        "case {case:?}: the accepted start must record one replica, got {held}"
    );
}

/// Every other operation of this service that takes an `attributes` map measures the map
/// it stores, so the same two halves sent as ONE map are refused there too: the composed
/// total this suite bounds is the same total those operations bound.
#[tokio::test]
async fn the_same_total_is_refused_as_one_map_elsewhere() {
    let emulator = RealTime::new();
    let case = "the composed total as one map";
    let stage_arn = emulator.stage(case, "one-map").await;
    let mut one_map = attributes("t", 2, 600);
    one_map.extend(attributes("r", 2, 600));
    let size = size_of(&one_map);

    let probes: Vec<(&str, Value)> = vec![
        (
            "CreateParticipantToken",
            json!({ "stageArn": stage_arn, "attributes": one_map }),
        ),
        (
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMPS", "attributes": one_map }),
        ),
    ];

    for (operation, request) in probes {
        let (status, response) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} must refuse {size} bytes, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: {operation} must answer ValidationException, got {response}"
        );
        let stated = member(&response, "message");
        let expected = expected_message(size);
        assert_eq!(
            stated, expected,
            "case {case:?}: {operation} must state {expected:?}, got {stated:?}"
        );
    }
}
