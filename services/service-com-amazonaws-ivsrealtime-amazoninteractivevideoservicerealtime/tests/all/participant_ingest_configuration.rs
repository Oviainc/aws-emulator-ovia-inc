// SPDX-License-Identifier: CC0-1.0

//! What a participant reports about the ingest configuration it broadcasts with, end to
//! end.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime get-participant` / `list-participants` call arrives, so
//! what is asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `Participant$ingestConfigurationArn` / `ParticipantSummary$ingestConfigurationArn` —
//! "the participant's ingest configuration" — names the configuration the participant
//! BROADCASTS with, resolved on every read against the configurations the store holds for
//! that stage: the one its join named, else the stage's configuration whose ingest
//! credentials were minted for that participant id. So it names the SAME configuration
//! `Participant$protocol` reports the `ingestProtocol` of, and the same one a read of the
//! configuration reports that participant as a publisher of (`IngestConfiguration$state`
//! `ACTIVE`); when the store holds no such configuration the member is absent, which is
//! how the model says "broadcasts with none".
//!
//! The rows differ in the identity the publisher reached the stage with (the
//! configuration's ARN, its primary ingest credential, its redundant one), in the ingest
//! protocol, in which projection is read (a full `GetParticipant` and the
//! `ListParticipants` summary of the same participant) and in whether a configuration of
//! that stage resolves the participant at all.
//!
//! Nothing in the IVS RealTime control plane makes a publisher BROADCAST to a stage: a
//! publisher reaches the stage over its data plane with the ingest key it was minted,
//! which is `state::join_stage_session` — the one place a participant record is created.
//! So these cases bring publishers into a session through it, reaching the store through
//! this suite's own engine handle, exactly as the `GetParticipant` suite beside it does.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The two values the modelled `IngestProtocol` carries, from
/// `smithy_query(query="shape",shape="IngestProtocol")` — each one is a row below.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

/// The value the modelled `ParticipantProtocol` carries for a participant no broadcast
/// protocol is known for, from `smithy_query(query="shape",shape="ParticipantProtocol")`.
const PROTOCOL_UNKNOWN: &str = "UNKNOWN";

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// How a row puts a publisher of a configuration into its stage: the three identities a
/// publisher of ONE configuration can carry.
#[derive(Debug, Clone, Copy)]
enum PublisherIdentity {
    /// Recorded with the configuration's ARN, as a broadcast over its ingest endpoint is.
    ConfigurationArn,
    /// Publishing as the participant the configuration's own ingest credential was minted
    /// for, naming no ARN on the join.
    PrimaryCredential,
    /// Publishing as the participant one of its redundant ingest credentials was minted
    /// for, naming no ARN on the join.
    RedundantCredential,
}

/// Every identity, in one list, so each test spans the whole class.
const PUBLISHER_IDENTITIES: [PublisherIdentity; 3] = [
    PublisherIdentity::ConfigurationArn,
    PublisherIdentity::PrimaryCredential,
    PublisherIdentity::RedundantCredential,
];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
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
             Credential=AKIDEXAMPLE/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a stage named `name` and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an ingest configuration of `stage_arn` broadcasting with `protocol`, and
    /// return the `ingestConfiguration` the service reported.
    async fn create_ingest_configuration(
        &self,
        case: &str,
        stage_arn: &str,
        protocol: &str,
        redundant: bool,
    ) -> Value {
        // `insecureIngest` must be true for the plaintext protocol, as
        // `CreateIngestConfiguration` itself refuses otherwise.
        let request = json!({
            "ingestProtocol": protocol,
            "stageArn": stage_arn,
            "insecureIngest": protocol == "RTMP",
            "redundantIngest": redundant,
        });
        let (status, response) = self.call("CreateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
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

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
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

    /// The `participant` a successful `GetParticipant` reported.
    async fn participant(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let (status, response) = self
            .call(
                "GetParticipant",
                &json!({
                    "stageArn": stage_arn,
                    "sessionId": session_id,
                    "participantId": participant_id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant on {participant_id:?} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participant"].clone()
    }

    /// The `ParticipantSummary` `ListParticipants` reports for one participant, so a read
    /// of one participant can be held against the page it appears in.
    async fn summary(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let (status, response) = self
            .call(
                "ListParticipants",
                &json!({ "stageArn": stage_arn, "sessionId": session_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants must answer HTTP 200, got {status} {response}"
        );
        response["participants"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .find(|summary| summary["participantId"].as_str() == Some(participant_id))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the session must list {participant_id:?}, got {response}")
            })
    }

    /// The `state` `GetIngestConfiguration` reports for one configuration — the read in
    /// the other direction, which says whether that participant publishes with it.
    async fn ingest_configuration_state(&self, case: &str, arn: &str) -> String {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["ingestConfiguration"], "state")
    }

    /// `DeleteIngestConfiguration` with `force`, which disconnects the publisher and
    /// removes the configuration from the store.
    async fn force_delete_ingest_configuration(&self, case: &str, arn: &str) {
        let (status, response) = self
            .call(
                "DeleteIngestConfiguration",
                &json!({ "arn": arn, "force": true }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a forced DeleteIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
    }
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

/// The participant id a publisher of `configuration` carries under `identity`, and the
/// `ingestConfigurationArn` its join records (none for the two credential identities: an
/// ingest credential names no configuration when it reaches the stage).
fn publisher_identity(
    configuration: &Value,
    identity: PublisherIdentity,
) -> (String, Option<String>) {
    match identity {
        PublisherIdentity::ConfigurationArn => (
            "broadcaster-1".to_string(),
            Some(member(configuration, "arn")),
        ),
        PublisherIdentity::PrimaryCredential => (member(configuration, "participantId"), None),
        PublisherIdentity::RedundantCredential => (
            member(
                &configuration["redundantIngestCredentials"][0],
                "participantId",
            ),
            None,
        ),
    }
}

/// Whatever identity a publisher reached the stage with, both participant projections name
/// the configuration it broadcasts with — the same one its `protocol` is the ingest
/// protocol of, and the same one that reports it as a publisher.
///
/// Each stage also holds a configuration the publisher is NOT an identity of, created
/// first and broadcasting with the other protocol, so a reading that named any
/// configuration of the stage would name that one instead.
#[tokio::test]
async fn every_publisher_identity_names_the_configuration_it_broadcasts_with() {
    for identity in PUBLISHER_IDENTITIES {
        for protocol in INGEST_PROTOCOLS {
            let case = format!("{identity:?} over {protocol}");
            let case = case.as_str();
            let emulator = Emulator::new();
            let stage_arn = emulator.create_stage(case, "ingested").await;
            // The decoy: another configuration of the same stage, with the other
            // protocol, whose credentials were minted for nobody in this session.
            let other_protocol = INGEST_PROTOCOLS
                .iter()
                .find(|candidate| **candidate != protocol)
                .expect("the model carries two ingest protocols");
            let decoy = emulator
                .create_ingest_configuration(case, &stage_arn, other_protocol, false)
                .await;
            let decoy_arn = member(&decoy, "arn");
            let configuration = emulator
                .create_ingest_configuration(case, &stage_arn, protocol, true)
                .await;
            let configuration_arn = member(&configuration, "arn");
            let (participant_id, named_on_join) = publisher_identity(&configuration, identity);
            assert!(
                !participant_id.is_empty() && !configuration_arn.is_empty(),
                "case {case:?}: the publishing participant must be identified, got {configuration}"
            );

            emulator.join(
                case,
                &stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.clone(),
                    published: true,
                    redundant_ingest: true,
                    ingest_configuration_arn: named_on_join,
                    ..Default::default()
                },
            );
            let session_id = emulator.active_session_id(case, &stage_arn).await;

            let participant = emulator
                .participant(case, &stage_arn, &session_id, &participant_id)
                .await;
            let reported = member(&participant, "ingestConfigurationArn");
            assert_eq!(
                reported, configuration_arn,
                "case {case:?}: expected the configuration the publisher broadcasts with, \
                 got {reported:?} in {participant}"
            );
            assert_ne!(
                reported, decoy_arn,
                "case {case:?}: the configuration the publisher is no identity of must not be \
                 named, got {participant}"
            );
            let reported_protocol = member(&participant, "protocol");
            assert_eq!(
                reported_protocol, protocol,
                "case {case:?}: expected the named configuration's ingest protocol, \
                 got {reported_protocol:?} in {participant}"
            );

            // The page's summary of the same participant answers alike.
            let summary = emulator
                .summary(case, &stage_arn, &session_id, &participant_id)
                .await;
            let summarised = member(&summary, "ingestConfigurationArn");
            assert_eq!(
                summarised, configuration_arn,
                "case {case:?}: expected the summary to name the same configuration as the \
                 read, got {summarised:?} in {summary}"
            );

            // And so does the read of the configuration itself, in the other direction.
            let publishing = emulator
                .ingest_configuration_state(case, &configuration_arn)
                .await;
            assert_eq!(
                publishing, PUBLISHING_STATE,
                "case {case:?}: expected {PUBLISHING_STATE:?} for the configuration the \
                 participant names, got {publishing:?}"
            );
            let idle = emulator.ingest_configuration_state(case, &decoy_arn).await;
            assert_eq!(
                idle, NO_PUBLISHER_STATE,
                "case {case:?}: expected {NO_PUBLISHER_STATE:?} for the configuration nobody \
                 publishes with, got {idle:?}"
            );
        }
    }
}

/// A participant no configuration of its stage resolves names none: the member is absent
/// and the protocol is the modelled `UNKNOWN`, in both projections.
///
/// The rows are the ways a participant can fail to be a publisher of its stage's
/// configurations: it holds a participant token, or it carries the participant id of a
/// configuration of ANOTHER stage.
#[tokio::test]
async fn a_participant_no_configuration_of_its_stage_resolves_names_none() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("setup", "ingested").await;
    let other_stage_arn = emulator.create_stage("setup", "elsewhere").await;
    // A configuration of each stage, so the stage under test HAS a configuration and the
    // foreign identity is a real one — of the wrong stage.
    emulator
        .create_ingest_configuration("setup", &stage_arn, "RTMPS", true)
        .await;
    let foreign = emulator
        .create_ingest_configuration("setup", &other_stage_arn, "RTMPS", true)
        .await;

    let cases = [
        ("a token holder of the stage", "token_holder-1".to_string()),
        (
            "the participant of another stage's configuration",
            member(&foreign, "participantId"),
        ),
        (
            "the redundant participant of another stage's configuration",
            member(&foreign["redundantIngestCredentials"][0], "participantId"),
        ),
    ];

    for (case, participant_id) in cases {
        assert!(
            !participant_id.is_empty(),
            "case {case:?}: the participant must be identified, got {foreign}"
        );
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                published: true,
                ..Default::default()
            },
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;

        let participant = emulator
            .participant(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert!(
            participant["ingestConfigurationArn"].is_null(),
            "case {case:?}: a participant broadcasting with no configuration of its stage must \
             name none, got {participant}"
        );
        let reported_protocol = member(&participant, "protocol");
        assert_eq!(
            reported_protocol, PROTOCOL_UNKNOWN,
            "case {case:?}: expected {PROTOCOL_UNKNOWN:?}, got {reported_protocol:?} in \
             {participant}"
        );
        let summary = emulator
            .summary(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert!(
            summary["ingestConfigurationArn"].is_null(),
            "case {case:?}: the summary must name none either, got {summary}"
        );
    }
}

/// The reading is a fact about the configurations the store holds, so every identity
/// answers alike as that store changes: a publisher that has LEFT the session still names
/// the configuration it broadcast with, and once that configuration is gone from the store
/// no participant names it — exactly as `protocol` falls back to the modelled `UNKNOWN`.
#[tokio::test]
async fn the_reported_configuration_follows_the_store_for_every_identity() {
    for identity in PUBLISHER_IDENTITIES {
        let case = format!("{identity:?}");
        let case = case.as_str();
        let emulator = Emulator::new();
        let stage_arn = emulator.create_stage(case, "ingested").await;
        let configuration = emulator
            .create_ingest_configuration(case, &stage_arn, "RTMPS", true)
            .await;
        let configuration_arn = member(&configuration, "arn");
        let (participant_id, named_on_join) = publisher_identity(&configuration, identity);
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                published: true,
                redundant_ingest: true,
                ingest_configuration_arn: named_on_join,
                ..Default::default()
            },
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;

        // A publisher that has left still names what it broadcast with.
        emulator.leave(case, &stage_arn, &session_id, &participant_id);
        let participant = emulator
            .participant(case, &stage_arn, &session_id, &participant_id)
            .await;
        let reported = member(&participant, "ingestConfigurationArn");
        assert_eq!(
            reported, configuration_arn,
            "case {case:?}: a participant that has left must still name the configuration it \
             broadcast with, got {reported:?} in {participant}"
        );

        // Once the configuration is gone from the store, nothing names it.
        emulator
            .force_delete_ingest_configuration(case, &configuration_arn)
            .await;
        let participant = emulator
            .participant(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert!(
            participant["ingestConfigurationArn"].is_null(),
            "case {case:?}: a configuration this store no longer holds must not be named, \
             got {participant}"
        );
        let reported_protocol = member(&participant, "protocol");
        assert_eq!(
            reported_protocol, PROTOCOL_UNKNOWN,
            "case {case:?}: expected {PROTOCOL_UNKNOWN:?} once the configuration is gone, \
             got {reported_protocol:?} in {participant}"
        );
        let summary = emulator
            .summary(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert!(
            summary["ingestConfigurationArn"].is_null(),
            "case {case:?}: the summary must stop naming it too, got {summary}"
        );
    }
}
