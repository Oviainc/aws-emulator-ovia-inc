// SPDX-License-Identifier: CC0-1.0

//! `GetParticipant` end to end: what the emulator reports for ONE participant of ONE
//! stage session.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs-realtime get-participant` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `GetParticipant` REPORTS the record the store holds for the `(stageArn, sessionId,
//! participantId)` triple the request names — the same record `ListParticipants`
//! reports a summary of, so the two agree member for member — with the two members
//! that are not stored values derived from the stage's own records on every read: the
//! S3 bucket and prefix the participant's individual recording is written to (the
//! storage configuration the stage's `autoParticipantRecordingConfiguration` names, or
//! the documented empty string when it names none) and the ingest protocol the
//! participant broadcasts with (the `ingestProtocol` of the ingest configuration it
//! publishes through, else the modelled `UNKNOWN`). A triple the store resolves no
//! participant for is refused with `ResourceNotFoundException` (HTTP 404). Being a
//! read, repeating any of it leaves every answer and every stored record exactly as it
//! was.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token it was
//! issued, which is `state::join_stage_session` — the one place a participant record is
//! created. So these cases bring participants into a session through it, reaching the
//! store through this suite's own engine handle, exactly as the `ListParticipants`
//! suite beside it does.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// The two values the modelled `IngestProtocol` carries, from
/// `smithy_query(query="shape",shape="IngestProtocol")` — every one of them is a row of
/// the protocol table below.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

/// The value the modelled `ParticipantProtocol` carries for a participant no broadcast
/// protocol is known for, from `smithy_query(query="shape",shape="ParticipantProtocol")`
/// (`UNKNOWN` | `WHIP` | `RTMP` | `RTMPS`).
const PROTOCOL_UNKNOWN: &str = "UNKNOWN";

/// `WHIP`, another value of that same enum: what a publisher reaching the stage over the
/// WHIP endpoint is seen broadcasting with.
const PROTOCOL_WHIP: &str = "WHIP";

/// A well-formed `StageSessionId` (`@length 16..16`, `@pattern ^st-[a-zA-Z0-9]+$`) that
/// no session of this store carries.
const ABSENT_SESSION_ID: &str = "st-0000000000000";

/// A well-formed `ParticipantId` (`@length 0..64`, `@pattern ^[a-zA-Z0-9-_]*$`) that no
/// participant of this store carries.
const ABSENT_PARTICIPANT_ID: &str = "nobody_joined-0";

/// A well-formed `StageArn` naming a stage this account does not hold.
const ABSENT_STAGE_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:stage/abcd1234";

/// One emulator holding only this service, driven through the router that serves
/// production traffic, plus the engine handle the data-plane seeding writes through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
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

    /// Create a stage from `request` and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, request: &Value) -> String {
        let (status, response) = self.call("CreateStage", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {request} must answer HTTP 200, got {status} {response}"
        );
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a storage configuration over `bucket` and return the ARN the service
    /// minted for it.
    async fn create_storage_configuration(&self, case: &str, bucket: &str) -> String {
        let request = json!({ "s3": { "bucketName": bucket }, "name": bucket });
        let (status, response) = self.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Issue a participant token for `stage_arn` and return the `participantToken` the
    /// service reported — the identity its holder joins the stage under.
    async fn create_token(&self, case: &str, request: &Value) -> Value {
        let (status, response) = self.call("CreateParticipantToken", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participantToken"].clone()
    }

    /// Create an ingest configuration from `request` and return the
    /// `ingestConfiguration` the service reported.
    async fn create_ingest_configuration(&self, case: &str, request: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", request).await;
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
        response["stage"]["activeSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
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

    /// `GetParticipant` for one triple: the status and the body a client sees.
    async fn get(&self, stage_arn: &str, session_id: &str, participant_id: &str) -> (u16, Value) {
        self.call(
            "GetParticipant",
            &json!({
                "stageArn": stage_arn,
                "sessionId": session_id,
                "participantId": participant_id,
            }),
        )
        .await
    }

    /// The `participant` a successful `GetParticipant` reported.
    async fn participant(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let (status, response) = self.get(stage_arn, session_id, participant_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant on {participant_id:?} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participant"].clone()
    }

    /// The `ParticipantSummary` `ListParticipants` reports for one participant of a
    /// session, so a read of one participant can be held against the page it appears in.
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
                panic!(
                    "case {case:?}: the session must list {participant_id:?}, got {response}"
                )
            })
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// Whether the whole error body names `fragment` in any of its strings.
///
/// Which wire key carries the text is the service's own envelope, so this looks at every
/// string of the body rather than pinning one key.
fn error_states(response: &Value, fragment: &str) -> bool {
    response.as_object().is_some_and(|members| {
        members
            .values()
            .any(|value| value.as_str().is_some_and(|text| text.contains(fragment)))
    })
}

/// One participant a case brings into a session, and what the data plane observed about
/// it.
struct Joining {
    user_id: Option<&'static str>,
    published: bool,
    recording_state: Option<&'static str>,
    attributes: Vec<(&'static str, &'static str)>,
    client: state::ParticipantClient,
    protocol: Option<&'static str>,
}

impl Joining {
    /// A participant that joins with nothing observed about it beyond its identity.
    fn plain(user_id: Option<&'static str>, published: bool) -> Self {
        Self {
            user_id,
            published,
            recording_state: None,
            attributes: Vec::new(),
            client: state::ParticipantClient::default(),
            protocol: None,
        }
    }

    fn with_attributes(mut self, attributes: &[(&'static str, &'static str)]) -> Self {
        self.attributes = attributes.to_vec();
        self
    }

    fn with_recording_state(mut self, recording_state: &'static str) -> Self {
        self.recording_state = Some(recording_state);
        self
    }

    fn with_client(mut self, client: state::ParticipantClient) -> Self {
        self.client = client;
        self
    }

    fn with_protocol(mut self, protocol: &'static str) -> Self {
        self.protocol = Some(protocol);
        self
    }

    fn attribute_map(&self) -> HashMap<String, String> {
        self.attributes
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// Issue a token carrying `joining`'s identity, record it joining `stage_arn`, and return
/// the participant id it joined under.
async fn seed(emulator: &Emulator, case: &str, stage_arn: &str, joining: &Joining) -> String {
    let mut request = json!({ "stageArn": stage_arn });
    if let Some(user_id) = joining.user_id {
        request["userId"] = json!(user_id);
    }
    if !joining.attributes.is_empty() {
        request["attributes"] = json!(joining.attribute_map());
    }
    let token = emulator.create_token(case, &request).await;
    let participant_id = token["participantId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    emulator.join(
        case,
        stage_arn,
        state::JoinedParticipant {
            participant_id: participant_id.clone(),
            user_id: joining.user_id.map(str::to_string),
            attributes: joining.attribute_map(),
            published: joining.published,
            recording_state: joining.recording_state.map(str::to_string),
            client: joining.client.clone(),
            protocol: joining.protocol.map(str::to_string),
            ..Default::default()
        },
    );
    participant_id
}

/// A client the data plane observed in full: every one of the six members `Participant`
/// carries about the joining client.
fn full_client() -> state::ParticipantClient {
    state::ParticipantClient {
        isp_name: Some("Example Networks".to_string()),
        os_name: Some("iOS".to_string()),
        os_version: Some("17.4.1".to_string()),
        browser_name: Some("Safari".to_string()),
        browser_version: Some("17.4".to_string()),
        sdk_version: Some("1.12.0".to_string()),
    }
}

/// A client only partly observed: the two members a native SDK with no browser reports.
fn partial_client() -> state::ParticipantClient {
    state::ParticipantClient {
        os_name: Some("Android".to_string()),
        sdk_version: Some("1.9.2".to_string()),
        ..Default::default()
    }
}

/// What a case expects `GetParticipant` to report for one of the six client members.
fn assert_client_member(case: &str, participant: &Value, member: &str, expected: Option<&str>) {
    let actual = participant[member].as_str();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {member} to be {expected:?}, got {actual:?} in {participant}"
    );
}

// ---------------------------------------------------------------------------
// What a read reports
// ---------------------------------------------------------------------------

/// Every modelled member of `Participant` carries what the join recorded, over
/// participants that differ in identity, in what they did in the session and in what the
/// data plane observed about them.
#[tokio::test]
async fn get_participant_reports_what_the_join_recorded() {
    // Rows differ along what the rule generalises over: whether the token named a user
    // id, whether the participant published, how many attributes its identity carried,
    // how much of the client was observed, and whether a broadcast protocol was seen.
    let cases: Vec<(&str, Joining)> = vec![
        (
            "a subscriber with nothing observed",
            Joining::plain(Some("subscriber"), false),
        ),
        (
            "an anonymous subscriber",
            Joining::plain(None, false),
        ),
        (
            "a publisher whose client is fully observed",
            Joining::plain(Some("publisher"), true)
                .with_client(full_client())
                .with_protocol(PROTOCOL_WHIP)
                .with_recording_state("ACTIVE"),
        ),
        (
            "a publisher whose client is partly observed",
            Joining::plain(Some("native"), true).with_client(partial_client()),
        ),
        (
            "a participant carrying one attribute",
            Joining::plain(Some("tagged"), false).with_attributes(&[("seat", "1A")]),
        ),
        (
            "a participant carrying several attributes",
            Joining::plain(Some("tagged-more"), true).with_attributes(&[
                ("seat", "2B"),
                ("role", "host"),
                ("locale", "en-GB"),
            ]),
        ),
    ];

    for (case, joining) in cases {
        let emulator = Emulator::new();
        let stage_arn = emulator.create_stage(case, &json!({ "name": "read" })).await;
        let participant_id = seed(&emulator, case, &stage_arn, &joining).await;
        let session_id = emulator.active_session_id(case, &stage_arn).await;

        let participant = emulator
            .participant(case, &stage_arn, &session_id, &participant_id)
            .await;

        assert_eq!(
            participant["participantId"].as_str(),
            Some(participant_id.as_str()),
            "case {case:?}: expected the participant the request named, got {participant}"
        );
        assert_eq!(
            participant["userId"].as_str(),
            joining.user_id,
            "case {case:?}: expected userId {:?}, got {participant}",
            joining.user_id
        );
        assert_eq!(
            participant["state"].as_str(),
            Some("CONNECTED"),
            "case {case:?}: a participant that joined and has not left is CONNECTED, \
             got {participant}"
        );
        assert!(
            !participant["firstJoinTime"].is_null(),
            "case {case:?}: expected the instant the participant joined, got {participant}"
        );
        assert_eq!(
            participant["published"], json!(joining.published),
            "case {case:?}: expected published {}, got {participant}",
            joining.published
        );
        assert_eq!(
            participant["recordingState"].as_str(),
            // `DISABLED` is what the modelled `ParticipantRecordingState` carries for a
            // participant nothing records, which is what a join naming no state is.
            Some(joining.recording_state.unwrap_or("DISABLED")),
            "case {case:?}: expected the recording state the join recorded, got {participant}"
        );
        assert_eq!(
            participant["replicationType"].as_str(),
            // The documented default of `ParticipantSummary$replicationType`, which a
            // participant no replication touched carries.
            Some("NONE"),
            "case {case:?}: expected replicationType NONE, got {participant}"
        );

        // The attributes the identity carried are reported when it carried any, and the
        // member is absent when it carried none.
        if joining.attributes.is_empty() {
            assert!(
                participant["attributes"].is_null(),
                "case {case:?}: a participant carrying no attributes reports no \
                 attributes member, got {participant}"
            );
        } else {
            assert_eq!(
                participant["attributes"],
                json!(joining.attribute_map()),
                "case {case:?}: expected the attributes the identity carried, got {participant}"
            );
        }

        // Each of the six client members reports exactly what the data plane observed,
        // and is absent for one it did not observe.
        for (member, observed) in [
            ("ispName", joining.client.isp_name.as_deref()),
            ("osName", joining.client.os_name.as_deref()),
            ("osVersion", joining.client.os_version.as_deref()),
            ("browserName", joining.client.browser_name.as_deref()),
            ("browserVersion", joining.client.browser_version.as_deref()),
            ("sdkVersion", joining.client.sdk_version.as_deref()),
        ] {
            assert_client_member(case, &participant, member, observed);
        }

        // The protocol the data plane saw, and `UNKNOWN` when it saw none and the
        // participant broadcasts through no ingest configuration.
        assert_eq!(
            participant["protocol"].as_str(),
            Some(joining.protocol.unwrap_or(PROTOCOL_UNKNOWN)),
            "case {case:?}: expected the observed protocol, got {participant}"
        );

        // The one record, two readers: every member `ParticipantSummary` also carries
        // must match the page the participant appears in.
        let summary = emulator
            .summary(case, &stage_arn, &session_id, &participant_id)
            .await;
        for member in [
            "participantId",
            "userId",
            "state",
            "firstJoinTime",
            "published",
            "recordingState",
            "replicationType",
            "replicationState",
            "sourceStageArn",
            "sourceSessionId",
            "redundantIngest",
            "ingestConfigurationArn",
        ] {
            assert_eq!(
                participant[member], summary[member],
                "case {case:?}: GetParticipant and ListParticipants must agree on \
                 {member}, got {} and {}",
                participant[member], summary[member]
            );
        }
    }
}

/// A participant that leaves is still reported, as `DISCONNECTED`, with the instant it
/// FIRST joined unchanged by the departure and by a later rejoin.
#[tokio::test]
async fn get_participant_reports_a_participant_that_left_and_rejoined() {
    let case = "leave and rejoin";
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage(case, &json!({ "name": "revolving" })).await;
    // A second participant keeps the session open when the first one leaves, so the
    // rejoin lands in the SAME session and the two reads are of one record.
    let staying = seed(
        &emulator,
        case,
        &stage_arn,
        &Joining::plain(Some("staying"), false),
    )
    .await;
    let joining = Joining::plain(Some("revolving"), true).with_client(full_client());
    let participant_id = seed(&emulator, case, &stage_arn, &joining).await;
    let session_id = emulator.active_session_id(case, &stage_arn).await;

    let joined = emulator
        .participant(case, &stage_arn, &session_id, &participant_id)
        .await;
    let first_join_time = joined["firstJoinTime"].clone();

    emulator.leave(case, &stage_arn, &session_id, &participant_id);
    let left = emulator
        .participant(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        left["state"].as_str(),
        Some("DISCONNECTED"),
        "case {case:?}: a participant that left is DISCONNECTED, got {left}"
    );
    assert_eq!(
        left["firstJoinTime"], first_join_time,
        "case {case:?}: expected the first join instant {first_join_time} to be \
         unchanged by the departure, got {left}"
    );
    assert_eq!(
        left["published"], json!(true),
        "case {case:?}: `published` is whether the participant EVER published, \
         got {left}"
    );

    // A rejoin brings it back to CONNECTED on the same record: same id, same first join.
    // This connection observes only the client's new SDK version, so that member takes
    // over and every member it did not observe keeps what the first connection saw.
    emulator.join(
        case,
        &stage_arn,
        state::JoinedParticipant {
            participant_id: participant_id.clone(),
            user_id: Some("revolving".to_string()),
            client: state::ParticipantClient {
                sdk_version: Some("1.13.0".to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let rejoined = emulator
        .participant(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        rejoined["state"].as_str(),
        Some("CONNECTED"),
        "case {case:?}: a rejoined participant is CONNECTED again, got {rejoined}"
    );
    assert_eq!(
        rejoined["firstJoinTime"], first_join_time,
        "case {case:?}: expected the first join instant {first_join_time} to survive a \
         rejoin, got {rejoined}"
    );
    // The member this connection observed takes over; the five it did not observe keep
    // what the first connection saw, rather than being withdrawn.
    assert_client_member(case, &rejoined, "sdkVersion", Some("1.13.0"));
    for (member, first_seen) in [
        ("ispName", full_client().isp_name),
        ("osName", full_client().os_name),
        ("osVersion", full_client().os_version),
        ("browserName", full_client().browser_name),
        ("browserVersion", full_client().browser_version),
    ] {
        assert_client_member(case, &rejoined, member, first_seen.as_deref());
    }

    // The participant that never left is untouched by any of it.
    let untouched = emulator
        .participant(case, &stage_arn, &session_id, &staying)
        .await;
    assert_eq!(
        untouched["state"].as_str(),
        Some("CONNECTED"),
        "case {case:?}: the staying participant must be unaffected, got {untouched}"
    );
}

// ---------------------------------------------------------------------------
// The two members derived from the stage's own records
// ---------------------------------------------------------------------------

/// Where a participant is being recorded takes both halves of "individual participant
/// recording is enabled": the stage's `autoParticipantRecordingConfiguration` names the
/// storage configuration whose bucket the media lands in, and the recorder reports a
/// state other than `DISABLED` for that participant. Either half missing is the
/// documented empty string, on both members.
#[tokio::test]
async fn get_participant_reports_the_recording_destination_the_stage_configures() {
    let case = "recording destination";
    let emulator = Emulator::new();
    let first = emulator
        .create_storage_configuration(case, "first-recordings")
        .await;
    let second = emulator
        .create_storage_configuration(case, "second-recordings")
        .await;

    // Rows differ in WHICH configuration the stage records into, in whether it records at
    // all (the empty string is the documented "no storage configuration is specified"
    // value, and an absent member is the same thing left out), and in what the recorder
    // reports for the participant — `DISABLED` being the state of a participant nothing
    // records, whatever the stage configures.
    let cases: Vec<(&str, Value, Option<&'static str>, &str)> = vec![
        (
            "recording into the first bucket",
            json!({ "storageConfigurationArn": first.as_str() }),
            Some("ACTIVE"),
            "first-recordings",
        ),
        (
            "recording into the second bucket, still starting",
            json!({ "storageConfigurationArn": second.as_str() }),
            Some("STARTING"),
            "second-recordings",
        ),
        (
            "the documented empty string",
            json!({ "storageConfigurationArn": "" }),
            Some("ACTIVE"),
            "",
        ),
        (
            "no recording configuration at all",
            Value::Null,
            Some("ACTIVE"),
            "",
        ),
        (
            "a configured stage, but a participant nothing records",
            json!({ "storageConfigurationArn": first.as_str() }),
            None,
            "",
        ),
    ];

    for (row, configuration, recording_state, expected_bucket) in cases {
        let mut request = json!({ "name": "recorded" });
        if !configuration.is_null() {
            request["autoParticipantRecordingConfiguration"] = configuration;
        }
        let stage_arn = emulator.create_stage(row, &request).await;
        let mut first_joining = Joining::plain(Some("one"), true);
        let mut second_joining = Joining::plain(Some("two"), true);
        if let Some(recording_state) = recording_state {
            first_joining = first_joining.with_recording_state(recording_state);
            second_joining = second_joining.with_recording_state(recording_state);
        }
        let first_id = seed(&emulator, row, &stage_arn, &first_joining).await;
        let second_id = seed(&emulator, row, &stage_arn, &second_joining).await;
        let session_id = emulator.active_session_id(row, &stage_arn).await;

        let participant = emulator
            .participant(row, &stage_arn, &session_id, &first_id)
            .await;
        assert_eq!(
            participant["recordingS3BucketName"].as_str(),
            Some(expected_bucket),
            "case {row:?}: expected the bucket the stage records into, got {participant}"
        );

        let prefix = participant["recordingS3Prefix"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: expected a prefix, got {participant}"));
        if expected_bucket.is_empty() {
            assert_eq!(
                prefix, "",
                "case {row:?}: a participant nothing records carries the empty prefix, \
                 got {prefix:?}"
            );
        } else {
            // The member's modelled `@pattern ^[a-zA-Z0-9-]*$` and `@length 0..256`,
            // from `smithy_query(query="shape",shape="Participant")`.
            assert!(
                !prefix.is_empty()
                    && prefix.len() <= 256
                    && prefix
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "case {row:?}: expected a prefix the modelled pattern admits, got {prefix:?}"
            );
            // One prefix per participant, and the same prefix on every read of one.
            let again = emulator
                .participant(row, &stage_arn, &session_id, &first_id)
                .await;
            assert_eq!(
                again["recordingS3Prefix"].as_str(),
                Some(prefix),
                "case {row:?}: expected the prefix to be stable across reads, got {again}"
            );
            let other = emulator
                .participant(row, &stage_arn, &session_id, &second_id)
                .await;
            assert_ne!(
                other["recordingS3Prefix"].as_str(),
                Some(prefix),
                "case {row:?}: expected a prefix per participant, got {other}"
            );
        }
    }
}

/// The recording prefix satisfies its own modelled pattern whatever the participant id
/// carries: `ParticipantId` admits `_` and `recordingS3Prefix` does not.
#[tokio::test]
async fn the_recording_prefix_satisfies_its_pattern_for_every_participant_id() {
    let case = "prefix characters";
    let emulator = Emulator::new();
    let storage_arn = emulator
        .create_storage_configuration(case, "prefix-recordings")
        .await;
    let stage_arn = emulator
        .create_stage(
            case,
            &json!({
                "name": "prefixed",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": storage_arn.as_str(),
                },
            }),
        )
        .await;

    // Rows differ in the characters the id carries, each one the modelled `ParticipantId`
    // (`@length 0..64`, `@pattern ^[a-zA-Z0-9-_]*$`) admits.
    for participant_id in ["plain123", "with-dashes", "with_underscores", "mIxEd-_09"] {
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.to_string(),
                published: true,
                recording_state: Some("ACTIVE".to_string()),
                ..Default::default()
            },
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;
        let participant = emulator
            .participant(case, &stage_arn, &session_id, participant_id)
            .await;
        let prefix = participant["recordingS3Prefix"]
            .as_str()
            .unwrap_or_else(|| panic!("case {participant_id:?}: expected a prefix, got {participant}"));
        assert!(
            !prefix.is_empty()
                && prefix.len() <= 256
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {participant_id:?}: expected a prefix the modelled pattern admits, \
             got {prefix:?}"
        );
    }
}

/// A publisher broadcasting with an ingest configuration reports THAT configuration's
/// ingest protocol, over every value the model admits; one broadcasting with none
/// reports the modelled `UNKNOWN`.
#[tokio::test]
async fn get_participant_reports_the_protocol_the_participant_broadcasts_with() {
    for protocol in INGEST_PROTOCOLS {
        let case = protocol;
        let emulator = Emulator::new();
        let stage_arn = emulator
            .create_stage(case, &json!({ "name": "ingested" }))
            .await;
        let configuration = emulator
            .create_ingest_configuration(
                case,
                // `insecureIngest` must be true for the plaintext protocol, as
                // `CreateIngestConfiguration` itself refuses otherwise.
                &json!({
                    "ingestProtocol": protocol,
                    "stageArn": stage_arn.as_str(),
                    "insecureIngest": protocol == "RTMP",
                }),
            )
            .await;
        let configuration_arn = configuration["arn"].as_str().unwrap_or_default().to_string();
        let publisher_id = configuration["participantId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The publisher reaches the stage with that configuration's ingest credential,
        // which is what the data plane records on the join.
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: publisher_id.clone(),
                published: true,
                ingest_configuration_arn: Some(configuration_arn.clone()),
                ..Default::default()
            },
        );
        // And a token holder of the same stage broadcasts through nothing.
        let subscriber_id = seed(
            &emulator,
            case,
            &stage_arn,
            &Joining::plain(Some("subscriber"), false),
        )
        .await;
        let session_id = emulator.active_session_id(case, &stage_arn).await;

        let publisher = emulator
            .participant(case, &stage_arn, &session_id, &publisher_id)
            .await;
        assert_eq!(
            publisher["protocol"].as_str(),
            Some(protocol),
            "case {case:?}: expected the ingest configuration's protocol, got {publisher}"
        );
        assert_eq!(
            publisher["ingestConfigurationArn"].as_str(),
            Some(configuration_arn.as_str()),
            "case {case:?}: expected the configuration the publisher broadcast with, \
             got {publisher}"
        );

        let subscriber = emulator
            .participant(case, &stage_arn, &session_id, &subscriber_id)
            .await;
        assert_eq!(
            subscriber["protocol"].as_str(),
            Some(PROTOCOL_UNKNOWN),
            "case {case:?}: a participant broadcasting through nothing is UNKNOWN, \
             got {subscriber}"
        );
        assert!(
            subscriber["ingestConfigurationArn"].is_null(),
            "case {case:?}: a token holder names no ingest configuration, got {subscriber}"
        );
    }
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// Every triple the store resolves no participant for is refused with
/// `ResourceNotFoundException` (HTTP 404), naming the identifier that resolved to
/// nothing.
#[tokio::test]
async fn get_participant_refuses_a_triple_it_holds_no_participant_for() {
    let case = "unresolved triple";
    let emulator = Emulator::new();
    let stage_arn = emulator
        .create_stage(case, &json!({ "name": "present" }))
        .await;
    let other_stage_arn = emulator
        .create_stage(case, &json!({ "name": "elsewhere" }))
        .await;
    let participant_id = seed(
        &emulator,
        case,
        &stage_arn,
        &Joining::plain(Some("present"), false),
    )
    .await;
    let session_id = emulator.active_session_id(case, &stage_arn).await;

    // A participant of ANOTHER stage, in that stage's own session: neither its id nor
    // its session names anything in the stage under test.
    let elsewhere_id = seed(
        &emulator,
        case,
        &other_stage_arn,
        &Joining::plain(Some("elsewhere"), false),
    )
    .await;
    let elsewhere_session_id = emulator.active_session_id(case, &other_stage_arn).await;

    // Rows differ in WHICH of the three identifiers resolves to nothing, and in how it
    // fails to: an id this account never minted, and one that names a resource of
    // another stage or session.
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a stage this account does not hold",
            ABSENT_STAGE_ARN,
            session_id.as_str(),
            participant_id.as_str(),
            ABSENT_STAGE_ARN,
        ),
        (
            "a session this stage never had",
            stage_arn.as_str(),
            ABSENT_SESSION_ID,
            participant_id.as_str(),
            ABSENT_SESSION_ID,
        ),
        (
            "a session of another stage",
            stage_arn.as_str(),
            elsewhere_session_id.as_str(),
            participant_id.as_str(),
            elsewhere_session_id.as_str(),
        ),
        (
            "a participant that never joined",
            stage_arn.as_str(),
            session_id.as_str(),
            ABSENT_PARTICIPANT_ID,
            ABSENT_PARTICIPANT_ID,
        ),
        (
            "a participant of another stage",
            stage_arn.as_str(),
            session_id.as_str(),
            elsewhere_id.as_str(),
            elsewhere_id.as_str(),
        ),
    ];

    for (row, stage, session, participant, named) in cases {
        let (status, response) = emulator.get(stage, session, participant).await;
        assert_eq!(
            status, 404,
            "case {row:?}: expected 404 for ({stage}, {session}, {participant}), \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {row:?}: expected ResourceNotFoundException, got {response}"
        );
        assert!(
            error_states(&response, named),
            "case {row:?}: expected the refusal to name {named:?}, got {response}"
        );
    }

    // And the read that does resolve still answers, so no refusal above consumed it.
    let participant = emulator
        .participant(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        participant["participantId"].as_str(),
        Some(participant_id.as_str()),
        "case {case:?}: the resolvable triple must still answer, got {participant}"
    );
}

/// `GetParticipant` is a READ: repeating it answers identically, and changes nothing a
/// later caller — of it or of `ListParticipants` — would see.
#[tokio::test]
async fn get_participant_changes_nothing_a_later_caller_sees() {
    let case = "repeated read";
    let emulator = Emulator::new();
    let storage_arn = emulator
        .create_storage_configuration(case, "repeat-recordings")
        .await;
    let stage_arn = emulator
        .create_stage(
            case,
            &json!({
                "name": "repeat",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": storage_arn.as_str(),
                },
            }),
        )
        .await;
    let participant_id = seed(
        &emulator,
        case,
        &stage_arn,
        &Joining::plain(Some("repeat"), true)
            .with_client(full_client())
            .with_attributes(&[("seat", "3C")])
            .with_recording_state("ACTIVE"),
    )
    .await;
    let session_id = emulator.active_session_id(case, &stage_arn).await;

    let first = emulator
        .participant(case, &stage_arn, &session_id, &participant_id)
        .await;
    let listed_before = emulator
        .summary(case, &stage_arn, &session_id, &participant_id)
        .await;
    for read in 2..=4 {
        let again = emulator
            .participant(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert_eq!(
            again, first,
            "case {case:?}: read {read} must report what read 1 did ({first}), got {again}"
        );
    }
    let listed_after = emulator
        .summary(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: expected the reads to leave the listing as it was \
         ({listed_before}), got {listed_after}"
    );
    // A refused read changes nothing either.
    let (status, _) = emulator
        .get(&stage_arn, &session_id, ABSENT_PARTICIPANT_ID)
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: the refused read must still be a 404"
    );
    let listed_last = emulator
        .summary(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        listed_last, listed_before,
        "case {case:?}: expected the refused read to leave the listing as it was, \
         got {listed_last}"
    );
}
