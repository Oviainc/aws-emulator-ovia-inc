// SPDX-License-Identifier: CC0-1.0

//! What makes an ingest configuration ACTIVE, and what that state refuses.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime …` call arrives, so what is asserted is the emulator's
//! HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, exercised over a SET of data-plane scripts rather than on one
//! request: a configuration is `ACTIVE`, and the operations that refuse over that state
//! refuse, exactly while a participant that broadcasts with it is BOTH connected to the
//! configuration's stage and publishing media to it right now —
//! `IngestConfiguration$state` "is `ACTIVE` if a publisher currently is publishing to the
//! stage associated with the ingest configuration", a delete is refused only "if the
//! publisher is actively streaming to a stage" and "an IngestConfiguration that is active
//! cannot be updated". A participant that is merely connected — one that joined to
//! subscribe only, or one whose media has stopped — is not publishing, so the
//! configuration it joined with is `INACTIVE`, deletable without `force` and updatable.
//!
//! The three surfaces the rule shows on are checked against the same store at the same
//! instant: the state `GetIngestConfiguration` reports, the one
//! `ListIngestConfigurations` (and its `filterByState`) reports, and the refusals of
//! `DeleteIngestConfiguration` without `force` and of `UpdateIngestConfiguration`. The
//! participant's own occurrences (`ListParticipantEvents`) are the store's other record of
//! the same fact, so they are asserted to agree.
//!
//! Nothing in the IVS RealTime control plane makes a publisher BROADCAST to a stage: a
//! publisher reaches the stage over its data plane with the ingest key it was minted,
//! which is `state::join_stage_session` — the one place a participant record is created.
//! So these cases bring participants into a session through it, reaching the store through
//! this suite's own engine handle, exactly as the `DeleteIngestConfiguration` suite beside
//! this one does.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{state, EmulatedService};

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The value of the modelled `ParticipantState` a participant still in the stage carries,
/// from `smithy_query(query="shape",shape="ParticipantState")`.
const PARTICIPANT_CONNECTED: &str = "CONNECTED";

/// The occurrence names that begin and end one participant's media, from
/// `smithy_query(query="shape",shape="EventName")`.
const EVENT_PUBLISH_STARTED: &str = "PUBLISH_STARTED";
const EVENT_PUBLISH_STOPPED: &str = "PUBLISH_STOPPED";

/// One thing the stage's data plane does to the participant of a case.
#[derive(Debug, Clone, Copy)]
enum Act {
    /// It joins the session, publishing media or only subscribing.
    Join { published: bool },
    /// It leaves the session.
    Leave,
}

/// Which of the identities a configuration broadcasts under the case's participant
/// carries — the three `IngestConfigurationRecord::publishes_participant` admits.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// The participant the configuration's own ingest credential was minted for.
    PrimaryCredential,
    /// The participant one of its redundant ingest credentials was minted for.
    RedundantCredential,
    /// Some other participant whose join named the configuration's ARN.
    ConfigurationArn,
}

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
    /// scope naming `ivs` in the region every case here uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let region = "us-east-1";
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

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an ingest configuration on `stage_arn` and return the
    /// `ingestConfiguration` the service reported for it.
    async fn create_configuration(&self, case: &str, stage_arn: &str) -> Value {
        let (status, response) = self
            .call(
                "CreateIngestConfiguration",
                &json!({
                    "ingestProtocol": "RTMPS",
                    "stageArn": stage_arn,
                    "redundantIngest": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration on {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// The participant id of a fresh participant token of `stage_arn` — an identity this
    /// service minted that no ingest configuration broadcasts under.
    async fn token_participant(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken on {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
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
    fn join(
        &self,
        case: &str,
        stage_arn: &str,
        participant_id: &str,
        published: bool,
        configuration_arn: Option<String>,
    ) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published,
                    redundant_ingest: true,
                    ingest_configuration_arn: configuration_arn,
                    ..Default::default()
                },
            )
            .is_some(),
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

    /// The `state` `GetIngestConfiguration` reports for `arn`.
    async fn read_state(&self, case: &str, arn: &str) -> String {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["ingestConfiguration"], "state")
    }

    /// The `state` a `ListIngestConfigurations` page reports for `arn`.
    async fn listed_state(&self, case: &str, arn: &str) -> String {
        let (status, response) = self.call("ListIngestConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListIngestConfigurations must answer HTTP 200, \
             got {status} {response}"
        );
        let page = response["ingestConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        page.iter()
            .find(|summary| member(summary, "arn") == arn)
            .map(|summary| member(summary, "state"))
            .unwrap_or_else(|| panic!("case {case:?}: {arn} must be listed, got {response}"))
    }

    /// The ARNs `ListIngestConfigurations` reports for one value of `filterByState`.
    async fn filtered_arns(&self, case: &str, wanted: &str) -> Vec<String> {
        let (status, response) = self
            .call(
                "ListIngestConfigurations",
                &json!({ "filterByState": wanted }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListIngestConfigurations filterByState={wanted} must answer \
             HTTP 200, got {status} {response}"
        );
        response["ingestConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect()
    }

    /// The names `ListParticipantEvents` reports for one participant, in occurrence
    /// order.
    async fn event_names(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Vec<String> {
        let (status, response) = self
            .call(
                "ListParticipantEvents",
                &json!({
                    "stageArn": stage_arn,
                    "sessionId": session_id,
                    "participantId": participant_id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantEvents must answer HTTP 200, got {status} {response}"
        );
        response["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| member(event, "name"))
            .collect()
    }

    /// The state `ListParticipants` reports for one participant of one session, or the
    /// empty string when the session does not hold it.
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
        response["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|participant| member(participant, "participantId") == participant_id)
            .map(|participant| member(participant, "state"))
            .unwrap_or_default()
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

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// Whether the latest of the participant's publish occurrences says its media is running
/// — the store's OTHER record of "is it publishing right now", which the configuration's
/// reported state has to agree with.
fn publishing_per_events(names: &[String]) -> bool {
    names
        .iter()
        .rev()
        .find(|name| name.as_str() == EVENT_PUBLISH_STARTED || name.as_str() == EVENT_PUBLISH_STOPPED)
        .is_some_and(|name| name.as_str() == EVENT_PUBLISH_STARTED)
}

/// Play `script` against the participant of the case and assert the three surfaces agree
/// on `expected_state`.
async fn check(case: &str, identity: Identity, script: &[Act], expected_state: &str) {
    let emulator = RealTime::new();
    let stage_arn = emulator.create_stage(case, "ingest").await;
    let configuration = emulator.create_configuration(case, &stage_arn).await;
    let configuration_arn = member(&configuration, "arn");

    // The identity the case's participant carries, and whether its join names the
    // configuration's ARN.
    let (participant_id, configuration_of_join) = match identity {
        Identity::PrimaryCredential => (member(&configuration, "participantId"), None),
        Identity::RedundantCredential => (
            member(
                &configuration["redundantIngestCredentials"][0],
                "participantId",
            ),
            None,
        ),
        Identity::ConfigurationArn => (
            emulator.token_participant(case, &stage_arn).await,
            Some(configuration_arn.clone()),
        ),
    };
    assert!(
        !participant_id.is_empty(),
        "case {case:?}: the case's participant must be identified, got {configuration}"
    );

    // A session "ends after the last participant leaves the stage", so a script whose
    // participant leaves and comes back would otherwise be looking at two sessions: a
    // bystander that joins first and stays keeps ONE session open for the whole script. It
    // carries a participant token of the stage, which no ingest configuration broadcasts
    // under, so it is never a publisher of the configuration under test.
    let bystander = emulator.token_participant(case, &stage_arn).await;
    emulator.join(case, &stage_arn, &bystander, false, None);
    let session_id = emulator.active_session_id(case, &stage_arn).await;
    assert!(
        !session_id.is_empty(),
        "case {case:?}: the bystander's join must have opened a session of {stage_arn}"
    );

    for act in script {
        match act {
            Act::Join { published } => emulator.join(
                case,
                &stage_arn,
                &participant_id,
                *published,
                configuration_of_join.clone(),
            ),
            Act::Leave => emulator.leave(case, &stage_arn, &session_id, &participant_id),
        }
    }

    // 1. What a READ reports.
    let read = emulator.read_state(case, &configuration_arn).await;
    assert_eq!(
        read, expected_state,
        "case {case:?}: expected GetIngestConfiguration to report state \
         {expected_state:?}, got {read:?}"
    );

    // 2. What a LISTING reports — the same fact, so the summary and the read agree, and
    //    each value of `filterByState` selects accordingly.
    let listed = emulator.listed_state(case, &configuration_arn).await;
    assert_eq!(
        listed, expected_state,
        "case {case:?}: expected ListIngestConfigurations to report state \
         {expected_state:?}, got {listed:?}"
    );
    for wanted in [PUBLISHING_STATE, NO_PUBLISHER_STATE] {
        let arns = emulator.filtered_arns(case, wanted).await;
        let selected = arns.contains(&configuration_arn);
        assert_eq!(
            selected,
            wanted == expected_state,
            "case {case:?}: filterByState={wanted} must select {configuration_arn} only \
             when it reports that state ({expected_state:?}), got {arns:?}"
        );
    }

    // 3. What the participant's OWN occurrences say about its media, which the state has
    //    to agree with.
    let names = emulator
        .event_names(case, &stage_arn, &session_id, &participant_id)
        .await;
    assert_eq!(
        publishing_per_events(&names),
        expected_state == PUBLISHING_STATE,
        "case {case:?}: expected the occurrences {names:?} to agree with state \
         {expected_state:?}"
    );

    // 4. An update is refused exactly while the configuration is active, and otherwise
    //    moves the association — including to the modelled "no stage".
    let (status, response) = emulator
        .call(
            "UpdateIngestConfiguration",
            &json!({ "arn": configuration_arn, "stageArn": "" }),
        )
        .await;
    if expected_state == PUBLISHING_STATE {
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 from UpdateIngestConfiguration while a \
             publisher is broadcasting, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException from UpdateIngestConfiguration, \
             got {response}"
        );
        let still = emulator.read_state(case, &configuration_arn).await;
        assert_eq!(
            still, expected_state,
            "case {case:?}: the refused update must change nothing, got state {still:?}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateIngestConfiguration to answer HTTP 200 for a \
             configuration nobody is publishing with, got {status} {response}"
        );
        let moved = member(&response["ingestConfiguration"], "stageArn");
        assert_eq!(
            moved, "",
            "case {case:?}: expected the update to leave the configuration associated with \
             no stage, got {moved:?}"
        );
        // Put it back, so the delete below is judged on the same association the script
        // produced.
        let (status, response) = emulator
            .call(
                "UpdateIngestConfiguration",
                &json!({ "arn": configuration_arn, "stageArn": stage_arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the configuration to be re-associated with \
             {stage_arn}, got {status} {response}"
        );
    }

    // 5. A delete that does not ask for force is refused exactly while the configuration
    //    is active; otherwise it removes the configuration and leaves a merely connected
    //    participant where it was.
    let (status, response) = emulator
        .call(
            "DeleteIngestConfiguration",
            &json!({ "arn": configuration_arn }),
        )
        .await;
    if expected_state == PUBLISHING_STATE {
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 from DeleteIngestConfiguration while a \
             publisher is broadcasting, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException from DeleteIngestConfiguration, \
             got {response}"
        );
        let still = emulator.read_state(case, &configuration_arn).await;
        assert_eq!(
            still, expected_state,
            "case {case:?}: the refused delete must remove nothing, got state {still:?}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteIngestConfiguration to answer HTTP 200 for a \
             configuration nobody is publishing with, got {status} {response}"
        );
        let (status, response) = emulator
            .call("GetIngestConfiguration", &json!({ "arn": configuration_arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted configuration to be gone, \
             got {status} {response}"
        );
        // A delete without force disconnects nobody: a participant the script left in the
        // stage is still in it.
        let left_connected = matches!(script.last(), Some(Act::Join { .. }));
        if left_connected {
            let participant = emulator
                .participant_state(case, &stage_arn, &session_id, &participant_id)
                .await;
            assert_eq!(
                participant, PARTICIPANT_CONNECTED,
                "case {case:?}: expected the participant {PARTICIPANT_CONNECTED:?} after a \
                 delete that forced nothing, got {participant:?}"
            );
        }
    }
}

/// Only a participant that is publishing RIGHT NOW makes the configuration it broadcasts
/// with `ACTIVE` — over scripts that differ in what the data plane did to it, how often,
/// and so in whether its media is running when the configuration is read.
#[tokio::test]
async fn state_follows_whether_a_publisher_is_publishing_right_now() {
    // Rows differ along the script the configuration's publisher ran: whether it joined at
    // all, whether that join published, how many joins and departures it made, and
    // therefore whether its media is running at the end.
    let rows: [(&str, &[Act], &str); 7] = [
        ("nobody joined the stage", &[], NO_PUBLISHER_STATE),
        (
            "joined to subscribe only",
            &[Act::Join { published: false }],
            NO_PUBLISHER_STATE,
        ),
        (
            "joined publishing",
            &[Act::Join { published: true }],
            PUBLISHING_STATE,
        ),
        (
            "joined publishing, then left",
            &[Act::Join { published: true }, Act::Leave],
            NO_PUBLISHER_STATE,
        ),
        (
            "joined publishing, left, back as a subscriber",
            &[
                Act::Join { published: true },
                Act::Leave,
                Act::Join { published: false },
            ],
            NO_PUBLISHER_STATE,
        ),
        (
            "joined as a subscriber, left, back publishing",
            &[
                Act::Join { published: false },
                Act::Leave,
                Act::Join { published: true },
            ],
            PUBLISHING_STATE,
        ),
        (
            "joined publishing, left and back publishing twice",
            &[
                Act::Join { published: true },
                Act::Leave,
                Act::Join { published: true },
                Act::Leave,
                Act::Join { published: true },
            ],
            PUBLISHING_STATE,
        ),
    ];

    for (case, script, expected_state) in rows {
        check(case, Identity::PrimaryCredential, script, expected_state).await;
    }
}

/// The rule holds for every identity a configuration broadcasts under — its own ingest
/// credential, one of its redundant credentials, and a join that names its ARN — so
/// connection without media leaves the configuration `INACTIVE` under each, and media
/// makes it `ACTIVE` under each.
#[tokio::test]
async fn state_follows_publishing_under_every_identity_of_the_configuration() {
    let identities = [
        ("the configuration's own ingest credential", Identity::PrimaryCredential),
        ("a redundant ingest credential", Identity::RedundantCredential),
        ("a join naming the configuration's ARN", Identity::ConfigurationArn),
    ];

    for (identity_case, identity) in identities {
        for (published, expected_state) in [
            (false, NO_PUBLISHER_STATE),
            (true, PUBLISHING_STATE),
        ] {
            let case = format!("{identity_case} / published={published}");
            check(
                &case,
                identity,
                &[Act::Join { published }],
                expected_state,
            )
            .await;
        }
    }
}
