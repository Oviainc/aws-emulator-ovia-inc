// SPDX-License-Identifier: CC0-1.0

//! What a stage's deletion does to the ingest configurations that name it.
//!
//! Every case drives the service through the router's own `dispatch` with a restJson1
//! request, exactly as a signed `aws ivs-realtime delete-stage` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, exercised over a SET of inputs rather than on one request: a
//! reported association never names a stage the service no longer holds, so deleting a
//! stage leaves every ingest configuration associated with it reporting the modelled "no
//! stage" value (the empty string, the first alternative of
//! `IngestConfiguration$stageArn`'s own pattern), and leaves the configuration itself —
//! its ingest key, its participant identities, its name and its tags — untouched. A
//! configuration associated with ANOTHER stage, or with no stage at all, keeps what it
//! reported.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`: a configuration is
/// `ACTIVE` while a publisher is publishing to the stage it is associated with, and
/// `INACTIVE` otherwise.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The `stageArn` a configuration associated with no stage reports: the empty string,
/// the first alternative of the `IngestConfiguration$stageArn` pattern
/// `^^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$$` from
/// `smithy_query(query="shape",shape="IngestConfiguration")`.
const NO_ASSOCIATED_STAGE_ARN: &str = "";

/// The two values of the modelled `IngestProtocol` enum, from
/// `smithy_query(query="shape",shape="IngestProtocol")`.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
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
    /// scope naming `ivs`.
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

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an ingest configuration and return the `ingestConfiguration` the service
    /// reported for it.
    async fn create_ingest_configuration(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// The `ingestConfiguration` an ARN names, read back over the wire.
    async fn read_ingest_configuration(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does —
    /// nothing in the control plane makes a participant join.
    fn join(&self, case: &str, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
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

/// Which stage a case associates one ingest configuration with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Association {
    /// The stage the case deletes.
    Doomed,
    /// A second stage, which the case keeps.
    Kept,
    /// No stage: the member is left out of the creating request.
    None,
}

/// One ingest configuration a case creates.
#[derive(Debug, Clone, Copy)]
struct Configuration {
    name: &'static str,
    association: Association,
    protocol: &'static str,
    redundant_ingest: bool,
    /// Whether a publisher is broadcasting with this configuration when the stage is
    /// deleted — the condition the operation's own doc names ("if there are participants
    /// using the IngestConfiguration to publish to the stage").
    publishing: bool,
}

/// One configuration a case created: what it reported at creation, so the read after the
/// delete can be compared against it member by member.
struct Created {
    spec: Configuration,
    arn: String,
    reported: Value,
}

/// Create every configuration of `specs` against the two stages, bringing a publisher
/// into the stage for each one the spec marks as publishing.
async fn populate(
    emulator: &RealTime,
    case: &str,
    doomed: &str,
    kept: &str,
    specs: &[Configuration],
) -> Vec<Created> {
    let mut created = Vec::new();
    for spec in specs {
        // `insecureIngest` is required to be true for RTMP and false for RTMPS, which
        // `CreateIngestConfiguration` enforces, so it follows the protocol of the row.
        let mut request = json!({
            "name": spec.name,
            "ingestProtocol": spec.protocol,
            "insecureIngest": spec.protocol == "RTMP",
            "redundantIngest": spec.redundant_ingest,
        });
        match spec.association {
            Association::Doomed => request["stageArn"] = json!(doomed),
            Association::Kept => request["stageArn"] = json!(kept),
            // The member left out entirely: the other way a configuration ends up
            // associated with no stage.
            Association::None => {}
        }
        let reported = emulator.create_ingest_configuration(case, &request).await;
        let arn = member(&reported, "arn");

        if spec.publishing {
            let stage_arn = match spec.association {
                Association::Doomed => doomed,
                Association::Kept => kept,
                Association::None => {
                    panic!("case {case:?}: a configuration with no stage cannot publish")
                }
            };
            emulator.join(
                case,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: member(&reported, "participantId"),
                    published: true,
                    redundant_ingest: spec.redundant_ingest,
                    ingest_configuration_arn: Some(arn.clone()),
                    ..Default::default()
                },
            );
        }

        created.push(Created {
            spec: *spec,
            arn,
            reported,
        });
    }
    created
}

/// Assert that the configuration `created` names reports `expected_stage_arn` and has
/// kept everything else it was created with.
async fn assert_configuration(
    emulator: &RealTime,
    case: &str,
    created: &Created,
    expected_stage_arn: &str,
    expected_state: &str,
) {
    let read = emulator.read_ingest_configuration(case, &created.arn).await;
    let name = created.spec.name;
    let actual_stage_arn = member(&read, "stageArn");
    assert_eq!(
        actual_stage_arn, expected_stage_arn,
        "case {case:?}: expected configuration {name:?} to report stageArn \
         {expected_stage_arn:?}, got {actual_stage_arn:?} ({read})"
    );
    let actual_state = member(&read, "state");
    assert_eq!(
        actual_state, expected_state,
        "case {case:?}: expected configuration {name:?} to report state \
         {expected_state:?}, got {actual_state:?} ({read})"
    );
    // The configuration itself survives the stage: only the association is dropped, so
    // every other member reads back exactly as it was created.
    for key in [
        "arn",
        "name",
        "ingestProtocol",
        "streamKey",
        "participantId",
    ] {
        let before = member(&created.reported, key);
        let after = member(&read, key);
        assert_eq!(
            after, before,
            "case {case:?}: expected configuration {name:?} to keep {key} {before:?}, \
             got {after:?} ({read})"
        );
    }
    assert_eq!(
        read["redundantIngest"], created.reported["redundantIngest"],
        "case {case:?}: expected configuration {name:?} to keep redundantIngest {}, got {} ({read})",
        created.reported["redundantIngest"], read["redundantIngest"]
    );
    assert_eq!(
        read["redundantIngestCredentials"], created.reported["redundantIngestCredentials"],
        "case {case:?}: expected configuration {name:?} to keep its redundant ingest \
         credentials {}, got {} ({read})",
        created.reported["redundantIngestCredentials"], read["redundantIngestCredentials"]
    );
}

/// Deleting a stage leaves every ingest configuration associated with it reporting no
/// stage, and leaves every other configuration exactly as it was — over rows differing in
/// how many configurations name the deleted stage, whether a publisher is broadcasting
/// with each of them, which ingest protocol they use, whether they take redundant ingest,
/// and whether other configurations name the surviving stage or no stage at all.
#[tokio::test]
async fn deleting_a_stage_clears_it_from_every_configuration_associated_with_it() {
    let cases: Vec<(&str, Vec<Configuration>)> = vec![
        (
            "one associated configuration, nobody publishing",
            vec![Configuration {
                name: "lone",
                association: Association::Doomed,
                protocol: INGEST_PROTOCOLS[1],
                redundant_ingest: false,
                publishing: false,
            }],
        ),
        (
            "one associated configuration with a publisher broadcasting right now",
            vec![Configuration {
                name: "publishing",
                association: Association::Doomed,
                protocol: INGEST_PROTOCOLS[0],
                redundant_ingest: false,
                publishing: true,
            }],
        ),
        (
            "a redundant-ingest configuration whose publisher is broadcasting",
            vec![Configuration {
                name: "redundant",
                association: Association::Doomed,
                protocol: INGEST_PROTOCOLS[1],
                redundant_ingest: true,
                publishing: true,
            }],
        ),
        (
            "three associated configurations beside one on the kept stage and one with \
             no stage",
            vec![
                Configuration {
                    name: "doomed-idle",
                    association: Association::Doomed,
                    protocol: INGEST_PROTOCOLS[0],
                    redundant_ingest: false,
                    publishing: false,
                },
                Configuration {
                    name: "doomed-publishing",
                    association: Association::Doomed,
                    protocol: INGEST_PROTOCOLS[1],
                    redundant_ingest: true,
                    publishing: true,
                },
                Configuration {
                    name: "doomed-redundant-idle",
                    association: Association::Doomed,
                    protocol: INGEST_PROTOCOLS[1],
                    redundant_ingest: true,
                    publishing: false,
                },
                Configuration {
                    name: "kept-publishing",
                    association: Association::Kept,
                    protocol: INGEST_PROTOCOLS[0],
                    redundant_ingest: false,
                    publishing: true,
                },
                Configuration {
                    name: "unassociated",
                    association: Association::None,
                    protocol: INGEST_PROTOCOLS[1],
                    redundant_ingest: false,
                    publishing: false,
                },
            ],
        ),
        (
            "no configuration names the deleted stage",
            vec![
                Configuration {
                    name: "kept-idle",
                    association: Association::Kept,
                    protocol: INGEST_PROTOCOLS[0],
                    redundant_ingest: false,
                    publishing: false,
                },
                Configuration {
                    name: "unassociated",
                    association: Association::None,
                    protocol: INGEST_PROTOCOLS[0],
                    redundant_ingest: true,
                    publishing: false,
                },
            ],
        ),
        ("no configuration at all", vec![]),
    ];

    for (case, specs) in cases {
        let emulator = RealTime::new();
        let doomed = emulator.create_stage(case, "doomed").await;
        let kept = emulator.create_stage(case, "kept").await;
        let created = populate(&emulator, case, &doomed, &kept, &specs).await;

        // Before the delete, each configuration reports the stage it was created with,
        // and `ACTIVE` exactly while a publisher is broadcasting with it.
        for configuration in &created {
            let expected_stage_arn = match configuration.spec.association {
                Association::Doomed => doomed.as_str(),
                Association::Kept => kept.as_str(),
                Association::None => NO_ASSOCIATED_STAGE_ARN,
            };
            let expected_state = if configuration.spec.publishing {
                PUBLISHING_STATE
            } else {
                NO_PUBLISHER_STATE
            };
            assert_configuration(
                &emulator,
                case,
                configuration,
                expected_stage_arn,
                expected_state,
            )
            .await;
        }

        let (status, response) = emulator.call("DeleteStage", &json!({ "arn": doomed })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {doomed}, got {status} {response}"
        );

        for configuration in &created {
            // The deleted stage is gone from every configuration that named it; the ones
            // naming the surviving stage, or no stage, are untouched.
            let (expected_stage_arn, expected_state) = match configuration.spec.association {
                Association::Doomed => (NO_ASSOCIATED_STAGE_ARN, NO_PUBLISHER_STATE),
                Association::Kept => (
                    kept.as_str(),
                    if configuration.spec.publishing {
                        PUBLISHING_STATE
                    } else {
                        NO_PUBLISHER_STATE
                    },
                ),
                Association::None => (NO_ASSOCIATED_STAGE_ARN, NO_PUBLISHER_STATE),
            };
            assert_configuration(
                &emulator,
                case,
                configuration,
                expected_stage_arn,
                expected_state,
            )
            .await;
        }

        // The stage itself really is gone, so the cleared association names nothing a
        // caller could still read.
        let (status, response) = emulator.call("GetStage", &json!({ "arn": doomed })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 reading the deleted {doomed}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for the deleted stage, \
             got {response}"
        );
    }
}

/// A configuration that lost its stage has no publisher left, so it deletes without
/// `force` — the publisher went with the stage — while one still publishing to a
/// surviving stage is refused, over both ingest protocols.
#[tokio::test]
async fn a_configuration_that_lost_its_stage_deletes_without_force() {
    for protocol in INGEST_PROTOCOLS {
        let case = format!("{protocol} publisher whose stage was deleted");
        let case = case.as_str();
        let emulator = RealTime::new();
        let doomed = emulator.create_stage(case, "doomed").await;
        let kept = emulator.create_stage(case, "kept").await;
        let specs = vec![
            Configuration {
                name: "on-the-doomed-stage",
                association: Association::Doomed,
                protocol,
                redundant_ingest: false,
                publishing: true,
            },
            Configuration {
                name: "on-the-kept-stage",
                association: Association::Kept,
                protocol,
                redundant_ingest: false,
                publishing: true,
            },
        ];
        let created = populate(&emulator, case, &doomed, &kept, &specs).await;

        let (status, response) = emulator.call("DeleteStage", &json!({ "arn": doomed })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {doomed}, got {status} {response}"
        );

        // The configuration whose stage went away has no publisher, so an unforced delete
        // is accepted.
        let orphaned = &created[0];
        let (status, response) = emulator
            .call(
                "DeleteIngestConfiguration",
                &json!({ "arn": orphaned.arn, "force": false }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting the disassociated {}, \
             got {status} {response}",
            orphaned.arn
        );

        // The one still publishing to the surviving stage is still refused without
        // `force`, so the delete above was not loosened for everything.
        let publishing = &created[1];
        let (status, response) = emulator
            .call(
                "DeleteIngestConfiguration",
                &json!({ "arn": publishing.arn, "force": false }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 deleting the still-publishing {}, \
             got {status} {response}",
            publishing.arn
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException for the still-publishing {}, \
             got {response}",
            publishing.arn
        );
    }
}
