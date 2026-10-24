// SPDX-License-Identifier: CC0-1.0

//! `DeleteIngestConfiguration` end to end: what the emulator does with the ingest
//! configuration an ARN names.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime delete-ingest-configuration` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULES under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A delete REMOVES the configuration its ARN names from the store, answers the
//!    modelled empty 200, and leaves every other configuration where it was — so a later
//!    `GetIngestConfiguration` on that ARN, and a second delete of it, answer the modelled
//!    `ResourceNotFoundException` (HTTP 404).
//! 2. An ARN this store holds no configuration for is that same 404, `force` or not; a
//!    value outside the `arn` member's modelled `@pattern`/`@length` is the modelled
//!    `ValidationException` (HTTP 400).
//! 3. "An IngestConfiguration cannot be deleted if the publisher is actively streaming to
//!    a stage, unless force is set to true": while a publisher is broadcasting with the
//!    configuration, it reports `state` `ACTIVE`, a delete without `force` is the modelled
//!    `ConflictException` (HTTP 409) and nothing is removed; with `force` the publisher is
//!    disconnected from the stage and the configuration is then deleted.
//! 4. A participant that is NOT a publisher of this configuration right now — one that has
//!    left the session, one in another stage, one holding a participant token — leaves the
//!    configuration `INACTIVE` and deletable without `force`.
//!
//! Nothing in the IVS RealTime control plane makes a publisher BROADCAST to a stage: a
//! publisher reaches the stage over its data plane with the ingest key it was minted,
//! which is `state::join_stage_session` — the one place a participant record is created.
//! So these cases bring publishers into a session through it, reaching the store through
//! this suite's own engine handle, exactly as the `ListParticipants` suite beside this one
//! does.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{state, EmulatedService};

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The modelled `@length` maximum of `IngestConfigurationArn`, from
/// `smithy_query(query="operation",operation="DeleteIngestConfiguration")`.
const INGEST_CONFIGURATION_ARN_MAX_LENGTH: usize = 128;

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`: a configuration is
/// `ACTIVE` while a publisher is publishing to its stage, and `INACTIVE` otherwise.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The two values the modelled `ParticipantState` carries, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const PARTICIPANT_CONNECTED: &str = "CONNECTED";
const PARTICIPANT_DISCONNECTED: &str = "DISCONNECTED";

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
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com/{operation}"
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

    /// [`Self::call_in`] against the region every case that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// Create an ingest configuration and return the `ingestConfiguration` the service
    /// reported for it.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session_id(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// `GetIngestConfiguration` for `arn`: the status and the body a client sees.
    async fn get(&self, arn: &str) -> (u16, Value) {
        self.call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await
    }

    /// `DeleteIngestConfiguration` for `arn`, with `force` as the case varies it (absent
    /// when `None`, so the modelled default applies).
    async fn delete(&self, arn: &str, force: Option<bool>) -> (u16, Value) {
        let mut request = json!({ "arn": arn });
        if let Some(force) = force {
            request["force"] = json!(force);
        }
        self.call("DeleteIngestConfiguration", &request).await
    }

    /// Record a publisher joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant leaving a session, the way the stage's data plane does.
    fn leave(&self, case: &str, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "case {case:?}: the departure of {participant_id:?} must be recorded"
        );
    }

    /// The state `ListParticipants` reports for one participant of one session.
    async fn participant_state(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> String {
        let (status, response) = self
            .call(
                "ListParticipants",
                &json!({ "stageArn": stage_arn, "sessionId": session_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants on {session_id} must answer HTTP 200, \
             got {status} {response}"
        );
        let participants = response["participants"].as_array().cloned().unwrap_or_default();
        participants
            .iter()
            .find(|participant| member(participant, "participantId") == participant_id)
            .map(|participant| member(participant, "state"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {participant_id:?} must still be listed, got {response}")
            })
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

/// Does this answer carry no members, as the modelled `DeleteIngestConfigurationResponse`
/// (a structure with no members) requires?
fn carries_no_members(response: &Value) -> bool {
    response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty)
}

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The create requests this suite spans the class of deletable configurations with: each
/// row differs from the others in what the rule generalises over — the ingest protocol,
/// whether redundant ingest is on (so the record carries backup credentials too), which
/// optional members it carries, and whether the delete asks for `force`.
fn delete_cases() -> Vec<(&'static str, Value, Option<bool>)> {
    vec![
        (
            "the smallest request the model allows, force left to its default",
            json!({ "ingestProtocol": "RTMPS" }),
            None,
        ),
        (
            "every optional member supplied, force false",
            json!({
                "ingestProtocol": "RTMPS",
                "name": "studio-feed",
                "userId": "user-7",
                "attributes": { "seat": "left" },
                "tags": { "env": "test" },
            }),
            Some(false),
        ),
        (
            "cleartext RTMP ingest, force true with nobody publishing",
            json!({ "ingestProtocol": "RTMP", "insecureIngest": true, "name": "legacy" }),
            Some(true),
        ),
        (
            "redundant ingest enabled",
            json!({ "ingestProtocol": "RTMPS", "redundantIngest": true }),
            Some(false),
        ),
        (
            "an explicitly empty stage association",
            json!({ "ingestProtocol": "RTMPS", "stageArn": "" }),
            None,
        ),
    ]
}

// ---------------------------------------------------------------------------
// RULE 1 and 2: what a delete removes, and what it refuses
// ---------------------------------------------------------------------------

/// A delete removes the configuration its ARN names and nothing else: the call answers an
/// empty 200, the ARN then reads as not found, a second delete of it is not found too, and
/// every other configuration of the store is still reported exactly as it was.
#[tokio::test]
async fn a_delete_removes_the_configuration_its_arn_names() {
    let emulator = RealTime::new();
    // A stage, so a row may associate its configuration with one this store holds.
    let stage_arn = emulator.create_stage("the fixture stage", "deletions").await;
    // A configuration no case deletes, so every delete is checked against a store that
    // still holds something.
    let bystander = emulator
        .create("the bystander", &json!({ "ingestProtocol": "RTMPS", "name": "keep" }))
        .await;
    let bystander_arn = member(&bystander, "arn");

    let mut cases = delete_cases();
    cases.push((
        "associated with an existing stage",
        json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn }),
        Some(false),
    ));

    for (case, request, force) in cases {
        let created = emulator.create(case, &request).await;
        let configuration_arn = member(&created, "arn");
        assert!(
            configuration_arn.len() <= INGEST_CONFIGURATION_ARN_MAX_LENGTH,
            "case {case:?}: expected an arn within the modelled length, got {configuration_arn:?}"
        );

        let (status, response) = emulator.delete(&configuration_arn, force).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteIngestConfiguration on {configuration_arn} must answer \
             HTTP 200, got {status} {response}"
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no members, got {response}"
        );

        // Gone: the read and a second delete both answer the modelled not-found, stating
        // the identifier the request named.
        let expected_message = format!("Resource: {configuration_arn} not found");
        for (reader, (status, response)) in [
            ("GetIngestConfiguration", emulator.get(&configuration_arn).await),
            (
                "DeleteIngestConfiguration",
                emulator.delete(&configuration_arn, force).await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?}: expected {reader} on the deleted {configuration_arn} to answer \
                 HTTP 404, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected {reader} to answer ResourceNotFoundException, \
                 got {response}"
            );
            let message = member(&response, "message");
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {reader} to state {expected_message:?}, got {message:?}"
            );
        }

        // Nothing else moved.
        let (status, response) = emulator.get(&bystander_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bystander must still be readable, got {status} {response}"
        );
        assert_eq!(
            response["ingestConfiguration"], bystander,
            "case {case:?}: expected the bystander unchanged, expected {bystander}, got {response}"
        );
    }
}

/// An ARN this store holds no configuration for is the modelled `ResourceNotFoundException`
/// (HTTP 404) whatever shape of well-formed ARN it is, and whether or not the request asks
/// for `force` — a delete does not invent a resource to succeed on.
#[tokio::test]
async fn an_unheld_arn_is_a_resource_not_found() {
    let emulator = RealTime::new();
    // One live configuration, so the store is not simply empty.
    let live = emulator
        .create("a live configuration", &json!({ "ingestProtocol": "RTMPS" }))
        .await;
    let live_arn = member(&live, "arn");

    let cases = [
        (
            "never created, in the region the request names",
            arn("us-east-1", "ingest-configuration", "abcdef123456"),
        ),
        (
            "never created, in another region",
            arn("eu-west-2", "ingest-configuration", "zz0099887766"),
        ),
        (
            "an id as long as the modelled maximum allows",
            arn("us-east-1", "ingest-configuration", &"a".repeat(60)),
        ),
    ];

    for (case, identifier) in cases {
        for force in [None, Some(false), Some(true)] {
            let (status, response) = emulator.delete(&identifier, force).await;
            assert_eq!(
                status, 404,
                "case {case:?} force={force:?}: expected HTTP 404 for {identifier}, \
                 got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} force={force:?}: expected ResourceNotFoundException, got {response}"
            );
            let expected = format!("Resource: {identifier} not found");
            let message = member(&response, "message");
            assert_eq!(
                message, expected,
                "case {case:?} force={force:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The live configuration was never touched by any of it.
    let (status, response) = emulator.get(&live_arn).await;
    assert_eq!(
        status, 200,
        "the live configuration must still be readable, got {status} {response}"
    );
    assert_eq!(
        response["ingestConfiguration"], live,
        "expected the live configuration unchanged, expected {live}, got {response}"
    );
}

/// A value outside the `arn` member's modelled constraints is refused as a
/// `ValidationException` (HTTP 400) — not as a not-found, and not as a 501.
#[tokio::test]
async fn an_arn_outside_the_modelled_constraints_is_a_validation_failure() {
    let emulator = RealTime::new();
    let cases = [
        ("the empty string, under the @length minimum", String::new()),
        ("not an ARN at all", "ingest-configuration".to_string()),
        (
            "another resource type",
            arn("us-east-1", "stage", "abcdef123456"),
        ),
        (
            "an id carrying a character outside the pattern",
            arn("us-east-1", "ingest-configuration", "abc_def"),
        ),
        (
            "one character past the @length maximum",
            format!(
                "arn:aws:ivs:us-east-1:{ACCOUNT}:ingest-configuration/{}",
                "a".repeat(INGEST_CONFIGURATION_ARN_MAX_LENGTH)
            ),
        ),
    ];

    for (case, identifier) in cases {
        let (status, response) = emulator.delete(&identifier, Some(true)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {identifier:?}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let message = member(&response, "message");
        assert!(
            message.starts_with("DeleteIngestConfiguration.arn "),
            "case {case:?}: expected a refusal naming the operation and the member, \
             got {message:?}"
        );
    }

    // A request that omits the `@required` member is refused the same way.
    let (status, response) = emulator
        .call("DeleteIngestConfiguration", &json!({}))
        .await;
    assert_eq!(
        status, 400,
        "case \"arn omitted\": expected HTTP 400, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case \"arn omitted\": expected ValidationException, got {response}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3 and 4: a publisher actively streaming to the stage
// ---------------------------------------------------------------------------

/// How a case puts a publisher of a configuration into its stage: the three identities a
/// publisher of one configuration can carry.
#[derive(Debug, Clone, Copy)]
enum PublisherIdentity {
    /// Recorded with the configuration's ARN, as a broadcast over its ingest endpoint is.
    ConfigurationArn,
    /// Publishing as the participant the configuration's own ingest credential was minted
    /// for.
    PrimaryCredential,
    /// Publishing as the participant one of its redundant ingest credentials was minted
    /// for.
    RedundantCredential,
}

/// While a publisher is broadcasting with a configuration, the configuration reports
/// `ACTIVE`, a delete without `force` is the modelled `ConflictException` (HTTP 409) and
/// removes nothing; with `force` the publisher is disconnected from the stage and the
/// configuration is deleted. Exercised over every identity a publisher can carry.
#[tokio::test]
async fn a_publishing_configuration_is_refused_without_force_and_deleted_with_it() {
    for identity in [
        PublisherIdentity::ConfigurationArn,
        PublisherIdentity::PrimaryCredential,
        PublisherIdentity::RedundantCredential,
    ] {
        let case = format!("{identity:?}");
        let case = case.as_str();
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "publishing").await;
        let configuration = emulator
            .create(
                case,
                &json!({
                    "ingestProtocol": "RTMPS",
                    "stageArn": stage_arn,
                    "redundantIngest": true,
                }),
            )
            .await;
        let configuration_arn = member(&configuration, "arn");

        // Who is publishing, and under which of the three identities.
        let (participant_id, configuration_of_join) = match identity {
            PublisherIdentity::ConfigurationArn => (
                "broadcaster-1".to_string(),
                Some(configuration_arn.clone()),
            ),
            PublisherIdentity::PrimaryCredential => {
                (member(&configuration, "participantId"), None)
            }
            PublisherIdentity::RedundantCredential => (
                member(
                    &configuration["redundantIngestCredentials"][0],
                    "participantId",
                ),
                None,
            ),
        };
        assert!(
            !participant_id.is_empty(),
            "case {case:?}: the publishing participant must be identified, got {configuration}"
        );
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                published: true,
                redundant_ingest: true,
                ingest_configuration_arn: configuration_of_join,
                ..Default::default()
            },
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;
        assert!(
            !session_id.is_empty(),
            "case {case:?}: the join must have opened a session of {stage_arn}"
        );

        // The configuration is ACTIVE while that publisher is connected.
        let (status, response) = emulator.get(&configuration_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the publishing configuration must be readable, got {status} {response}"
        );
        let state_while_publishing = member(&response["ingestConfiguration"], "state");
        assert_eq!(
            state_while_publishing, PUBLISHING_STATE,
            "case {case:?}: expected state {PUBLISHING_STATE:?} while a publisher is \
             broadcasting, got {state_while_publishing:?}"
        );

        // A delete that does not ask for force is refused, and removes nothing.
        for force in [None, Some(false)] {
            let (status, response) = emulator.delete(&configuration_arn, force).await;
            assert_eq!(
                status, 409,
                "case {case:?} force={force:?}: expected HTTP 409 while a publisher is \
                 broadcasting, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ConflictException",
                "case {case:?} force={force:?}: expected ConflictException, got {response}"
            );
            let message = member(&response, "message");
            assert!(
                message.contains(&configuration_arn) && message.contains(&participant_id),
                "case {case:?} force={force:?}: expected a refusal naming the configuration \
                 and the publisher, got {message:?}"
            );
            let (status, read) = emulator.get(&configuration_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} force={force:?}: the refused delete must remove nothing, \
                 got {status} {read}"
            );
        }
        // And the publisher is still in the stage.
        let publisher_state = emulator
            .participant_state(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert_eq!(
            publisher_state, PARTICIPANT_CONNECTED,
            "case {case:?}: expected the publisher {PARTICIPANT_CONNECTED:?} after a refused \
             delete, got {publisher_state:?}"
        );

        // With force the publisher is disconnected from the stage, and the configuration
        // is then deleted.
        let (status, response) = emulator.delete(&configuration_arn, Some(true)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a forced delete to answer HTTP 200, got {status} {response}"
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no members, got {response}"
        );
        let publisher_state = emulator
            .participant_state(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert_eq!(
            publisher_state, PARTICIPANT_DISCONNECTED,
            "case {case:?}: expected the publisher {PARTICIPANT_DISCONNECTED:?} after a forced \
             delete, got {publisher_state:?}"
        );
        let (status, response) = emulator.get(&configuration_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the forced-deleted configuration to be gone, \
             got {status} {response}"
        );
    }
}

/// A participant that is not a publisher of this configuration right now does not block a
/// delete: one that has left the session, one in another stage, and the holder of a
/// participant token of the same stage all leave the configuration `INACTIVE` and
/// deletable without `force`.
#[tokio::test]
async fn a_participant_that_is_not_publishing_with_it_does_not_block_a_delete() {
    for case in [
        "the publisher left the session",
        "a publisher of another stage",
        "a token holder of the same stage",
    ] {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "inactive").await;
        let other_stage_arn = emulator.create_stage(case, "elsewhere").await;
        let configuration = emulator
            .create(
                case,
                &json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn }),
            )
            .await;
        let configuration_arn = member(&configuration, "arn");

        match case {
            "the publisher left the session" => {
                let participant_id = member(&configuration, "participantId");
                emulator.join(
                    case,
                    &stage_arn,
                    state::JoinedParticipant {
                        participant_id: participant_id.clone(),
                        published: true,
                        ingest_configuration_arn: Some(configuration_arn.clone()),
                        ..Default::default()
                    },
                );
                let session_id = emulator.active_session_id(case, &stage_arn).await;
                emulator.leave(case, &stage_arn, &session_id, &participant_id);
            }
            "a publisher of another stage" => {
                emulator.join(
                    case,
                    &other_stage_arn,
                    state::JoinedParticipant {
                        participant_id: "broadcaster-elsewhere".to_string(),
                        published: true,
                        ingest_configuration_arn: Some(configuration_arn.clone()),
                        ..Default::default()
                    },
                );
            }
            _ => {
                let (status, token) = emulator
                    .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateParticipantToken must answer HTTP 200, got {token}"
                );
                let participant_id = member(&token["participantToken"], "participantId");
                emulator.join(
                    case,
                    &stage_arn,
                    state::JoinedParticipant {
                        participant_id,
                        published: true,
                        ..Default::default()
                    },
                );
            }
        }

        // Nobody is publishing WITH THIS CONFIGURATION, so it reports INACTIVE …
        let (status, response) = emulator.get(&configuration_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the configuration must be readable, got {status} {response}"
        );
        let state = member(&response["ingestConfiguration"], "state");
        assert_eq!(
            state, NO_PUBLISHER_STATE,
            "case {case:?}: expected state {NO_PUBLISHER_STATE:?}, got {state:?}"
        );

        // … and a delete that does not ask for force succeeds.
        let (status, response) = emulator.delete(&configuration_arn, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer HTTP 200, got {status} {response}"
        );
        let (status, response) = emulator.get(&configuration_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted configuration to be gone, \
             got {status} {response}"
        );
    }
}
