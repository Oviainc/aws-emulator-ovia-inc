// SPDX-License-Identifier: CC0-1.0

//! One decision, and one wording, for EVERY set of members this service documents as
//! "only one of these per request".
//!
//! THE RULE: an operation whose model documents a set of members only one of which may be
//! given serves a request that carries one of them or none, and refuses one that carries
//! more with the modelled `ValidationException` (HTTP 400) whose text names the whole set
//! and then the members the request actually carried — `<path> is required to have at most
//! one of <set>, found <carried>`, each list in the order the model declares the members —
//! while reporting no page and starting nothing.
//!
//! Three such sets exist in this service, one per operation that has one, and this suite
//! drives the shared decision from each of them:
//!
//! * `ListParticipants`' four filters — "Only one of filterByUserId, filterByPublished,
//!   filterByState, or filterByRecordingState can be provided per request", from
//!   `smithy_query(query="operation",operation="ListParticipants")`;
//! * `ListIngestConfigurations`' two filters — "Only one filter (by stage ARN or by state)
//!   can be used at a time", from
//!   `smithy_query(query="operation",operation="ListIngestConfigurations")`;
//! * the two members of `StartComposition`'s `layout`, from
//!   `smithy_query(query="shape",shape="LayoutConfiguration")`, whose refusal names the
//!   nested path rather than the operation alone.
//!
//! Rows differ along what the rule generalises over: which set (so the path and the set in
//! the sentence), WHICH of its members the request carried and HOW MANY of them (every
//! pair, every triple and the whole set, for the set of four), the values they carried, and
//! — for a member whose documented default makes "absent" and "at the default" one value —
//! whether the value carries the filter at all (`filterByPublished` at `false`).
//!
//! Every case goes through the router's own `dispatch`, so each assertion is about what an
//! SDK client sees: the HTTP status, the error code and the JSON body.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// One IVS RealTime emulator over one shared engine, plus the engine handle the
/// data-plane seeding writes a participant join through (nothing in the control plane
/// makes a participant join a stage; see `tests/all/participants.rs`).
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
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

    /// `operation`, asserting the modelled success, and answering with the whole body.
    async fn succeed(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        response
    }

    /// The ARN a create minted, read out of `member`.
    async fn created_arn(&self, case: &str, operation: &str, body: &Value, member: &str) -> String {
        let response = self.succeed(case, operation, body).await;
        response[member]["arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {operation} must mint an arn, got {response}")
            })
            .to_string()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does, and
    /// answer with the session the join opened.
    async fn join(&self, case: &str, stage_arn: &str, user_id: &str, published: bool) -> String {
        let token = self
            .succeed(
                case,
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn, "userId": user_id }),
            )
            .await;
        let participant_id = token["participantToken"]["participantId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        {
            let service_state = state::service_state(&*self.engine).expect("this service's store");
            let mut store = service_state.lock();
            assert!(
                state::join_stage_session(
                    &mut store,
                    stage_arn,
                    state::JoinedParticipant {
                        participant_id: participant_id.clone(),
                        user_id: Some(user_id.to_string()),
                        published,
                        recording_state: published.then(|| "ACTIVE".to_string()),
                        attributes: HashMap::new(),
                        ..Default::default()
                    },
                )
                .is_some(),
                "case {case:?}: the join of {participant_id:?} must be recorded"
            );
        }
        let stage = self
            .succeed(case, "GetStage", &json!({ "arn": stage_arn }))
            .await;
        stage["stage"]["activeSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The error code a refusal states, or `"<absent>"` so a failing assertion names the
/// missing member rather than panicking inside the reader.
fn error_code(response: &Value) -> &str {
    response
        .get("Code")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The one sentence this service refuses a set with: the whole set, then what the request
/// carried, both in the order the model declares the members.
fn at_most_one_message(path: &str, exclusive: &[&str], carried: &[&str]) -> String {
    format!(
        "{path} is required to have at most one of {}, found {}",
        exclusive.join(", "),
        carried.join(", "),
    )
}

/// One set of members only one of which may be given, as a call to the operation that has
/// it.
struct ExclusiveSet {
    /// The label this set's cases name themselves with.
    case: &'static str,
    /// The operation a case calls.
    operation: &'static str,
    /// The path the refusal names — the operation, and for a nested set the member path
    /// under it.
    path: String,
    /// The whole set, in the order the model declares the members, each paired with a
    /// value that CARRIES that member.
    members: Vec<(&'static str, Value)>,
    /// The member the set lives under, for a nested set; `None` when the set's members are
    /// the operation's own input members.
    nested_under: Option<&'static str>,
    /// What every call to this operation carries besides the set's members.
    base: Value,
    /// The `@required` output member a served call reports, so a served row is a real
    /// answer rather than only a 200.
    reports: &'static str,
}

impl ExclusiveSet {
    /// The set's member names, in model order.
    fn names(&self) -> Vec<&'static str> {
        self.members.iter().map(|(member, _)| *member).collect()
    }

    /// A call to this set's operation carrying exactly `carried` of its members.
    fn request(&self, carried: &[(&'static str, Value)]) -> Value {
        let mut request = self.base.clone();
        match self.nested_under {
            None => {
                for (member, value) in carried {
                    request[*member] = value.clone();
                }
            }
            Some(outer) => {
                let mut nested = json!({});
                for (member, value) in carried {
                    nested[*member] = value.clone();
                }
                request[outer] = nested;
            }
        }
        request
    }
}

/// Every subset of `members` that carries at least two of them, each in the order
/// `members` lists them: the pairs, the triples and the whole set.
fn carrying_at_least_two(
    members: &[(&'static str, Value)],
) -> Vec<Vec<(&'static str, Value)>> {
    let mut subsets = Vec::new();
    for mask in 0u32..(1 << members.len()) {
        let carried: Vec<(&'static str, Value)> = members
            .iter()
            .enumerate()
            .filter(|(index, _)| (mask >> index) & 1 == 1)
            .map(|(_, (member, value))| (*member, value.clone()))
            .collect();
        if carried.len() >= 2 {
            subsets.push(carried);
        }
    }
    subsets
}

/// The sets this suite drives, over a store holding a stage with a live session, an
/// ingest configuration associated with that stage, and the resources a composition's
/// destination refers to.
async fn exclusive_sets(emulator: &RealTime) -> Vec<ExclusiveSet> {
    let case = "fixture";
    let stage_arn = emulator
        .created_arn(case, "CreateStage", &json!({ "name": "exclusive-sets" }), "stage")
        .await;
    let session_id = emulator.join(case, &stage_arn, "alice", true).await;
    emulator
        .succeed(
            case,
            "CreateIngestConfiguration",
            &json!({ "name": "ingest", "stageArn": stage_arn, "ingestProtocol": "RTMPS" }),
        )
        .await;
    let encoder_configuration_arn = emulator
        .created_arn(
            case,
            "CreateEncoderConfiguration",
            &json!({ "name": "enc" }),
            "encoderConfiguration",
        )
        .await;
    let storage_configuration_arn = emulator
        .created_arn(
            case,
            "CreateStorageConfiguration",
            &json!({ "s3": { "bucketName": "exclusive-sets" } }),
            "storageConfiguration",
        )
        .await;

    vec![
        ExclusiveSet {
            case: "the four ListParticipants filters",
            operation: "ListParticipants",
            path: "ListParticipants".to_string(),
            // The values are ones each filter admits: a user id the session holds, the
            // `true` that asks for "only show participants who published", and one value
            // of each modelled filter enum (`ParticipantState`,
            // `ParticipantRecordingFilterByRecordingState`).
            members: vec![
                ("filterByUserId", json!("alice")),
                ("filterByPublished", json!(true)),
                ("filterByState", json!("CONNECTED")),
                ("filterByRecordingState", json!("ACTIVE")),
            ],
            nested_under: None,
            base: json!({ "stageArn": stage_arn, "sessionId": session_id }),
            reports: "participants",
        },
        ExclusiveSet {
            case: "the two ListIngestConfigurations filters",
            operation: "ListIngestConfigurations",
            path: "ListIngestConfigurations".to_string(),
            members: vec![
                ("filterByStageArn", json!(stage_arn)),
                // `INACTIVE` is the state a configuration with no publisher reports, from
                // `smithy_query(query="shape",shape="IngestConfigurationState")`.
                ("filterByState", json!("INACTIVE")),
            ],
            nested_under: None,
            base: json!({}),
            reports: "ingestConfigurations",
        },
        ExclusiveSet {
            case: "the two StartComposition layout members",
            operation: "StartComposition",
            path: "StartComposition.layout".to_string(),
            // An empty configuration carries the member: it selects that layout and leaves
            // every one of its own members at its documented default.
            members: vec![("grid", json!({})), ("pip", json!({}))],
            nested_under: Some("layout"),
            base: json!({
                "stageArn": stage_arn,
                "destinations": [{
                    "s3": {
                        "storageConfigurationArn": storage_configuration_arn,
                        "encoderConfigurationArns": [encoder_configuration_arn],
                    },
                }],
            }),
            reports: "composition",
        },
        ExclusiveSet {
            case: "the two StartComposition layout members, each configured",
            operation: "StartComposition",
            path: "StartComposition.layout".to_string(),
            // The same set, carried with members configured rather than left empty: what
            // decides the refusal is which layout the request names, not what it sizes.
            members: vec![
                ("grid", json!({ "gridGap": 4, "videoAspectRatio": "SQUARE" })),
                ("pip", json!({ "pipPosition": "TOP_LEFT", "pipWidth": 320 })),
            ],
            nested_under: Some("layout"),
            base: json!({
                "stageArn": stage_arn,
                "destinations": [{
                    "s3": {
                        "storageConfigurationArn": storage_configuration_arn,
                        "encoderConfigurationArns": [encoder_configuration_arn],
                    },
                }],
            }),
            reports: "composition",
        },
    ]
}

/// A request carrying more than one member of a set is refused in the ONE wording, over
/// every set of this service and every way of carrying more than one of its members.
#[tokio::test]
async fn carrying_more_than_one_member_of_a_set_is_refused_in_one_wording() {
    let emulator = RealTime::new();
    let sets = exclusive_sets(&emulator).await;

    for set in &sets {
        let exclusive = set.names();
        for carried in carrying_at_least_two(&set.members) {
            let names: Vec<&str> = carried.iter().map(|(member, _)| *member).collect();
            let case = format!("{} / carrying {}", set.case, names.join(" + "));
            let request = set.request(&carried);
            let (status, response) = emulator.call(set.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {request}, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            let expected = at_most_one_message(&set.path, &exclusive, &names);
            assert_eq!(
                error_message(&response),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                error_message(&response)
            );
            assert!(
                response.get(set.reports).is_none(),
                "case {case:?}: a refused call reports no {}, got {response}",
                set.reports
            );
        }
    }

    // Nothing the refused starts asked for was started: the refusal happens before the
    // composition is minted, so the store holds none.
    let listed = emulator
        .succeed("refused starts", "ListCompositions", &json!({}))
        .await;
    assert_eq!(
        listed["compositions"].as_array().map(Vec::len),
        Some(0),
        "a refused StartComposition must start no composition, got {listed}"
    );
}

/// The other half of the rule: one member of a set, or none, is SERVED — so the refusal is
/// about carrying more than one and not about the members themselves. A member whose
/// documented default makes "absent" and "at the default" one value is not carried at that
/// default, so it is served beside another member of the set.
#[tokio::test]
async fn one_member_of_a_set_or_none_is_served() {
    let emulator = RealTime::new();
    let sets = exclusive_sets(&emulator).await;

    for set in &sets {
        // Rows: the set's members one at a time, and then none of them at all.
        for carried in set
            .members
            .iter()
            .map(|member| vec![member.clone()])
            .chain([Vec::new()])
        {
            let names: Vec<&str> = carried.iter().map(|(member, _)| *member).collect();
            let case = format!(
                "{} / carrying {}",
                set.case,
                if names.is_empty() {
                    "none of them".to_string()
                } else {
                    names.join(" + ")
                }
            );
            let request = set.request(&carried);
            let response = emulator.succeed(&case, set.operation, &request).await;
            assert!(
                response.get(set.reports).is_some(),
                "case {case:?}: a served call reports its {}, got {response}",
                set.reports
            );
        }
    }

    // `ListParticipants$filterByPublished` carries the model's `@default false`, so an
    // absent member and `false` are one value and `false` asks for no filtering at all:
    // it is not a second member carried, and the request is served. One row per other
    // member of that set.
    let participants = sets
        .iter()
        .find(|set| set.operation == "ListParticipants")
        .expect("the ListParticipants set is one of the sets above");
    for (member, value) in participants
        .members
        .iter()
        .filter(|(member, _)| *member != "filterByPublished")
    {
        let case = format!("filterByPublished false beside {member}");
        let mut request = participants.base.clone();
        request["filterByPublished"] = json!(false);
        request[*member] = value.clone();
        let response = emulator.succeed(&case, participants.operation, &request).await;
        assert!(
            response.get(participants.reports).is_some(),
            "case {case:?}: the documented default carries no filter, so the call is \
             served, got {response}"
        );
    }
}
