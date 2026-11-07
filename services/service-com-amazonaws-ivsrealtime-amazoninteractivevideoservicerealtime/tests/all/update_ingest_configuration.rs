// SPDX-License-Identifier: CC0-1.0

//! `UpdateIngestConfiguration` end to end: what the emulator does with the ingest
//! configuration an ARN names when a request moves it between stages.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime update-ingest-configuration` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULES under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. The two members this operation's own input names are the two it writes: the stage
//!    association, and whether redundant ingest is enabled (an absent `redundantIngest`
//!    carrying the modelled default `false`). Setting that switch materialises the backup
//!    credentials the feature takes, which are the configuration's from then on, and every
//!    other member of the configuration is left exactly as it was: the ingest protocol, the
//!    minted primary ingest key and participant id, the name, user id, attributes. The
//!    answer is the configuration `GetIngestConfiguration` then reports, member for member,
//!    so the write round-trips; repeating the same update answers the same thing.
//! 2. The association a request supplies is resolved the way a create resolves it: a
//!    stage this account holds is taken, the empty string and an absent member are the
//!    modelled "no stage", and a well-formed stage ARN no stage answers for is the
//!    modelled `ValidationException` (HTTP 400) that persists nothing.
//! 3. An ARN this store holds no configuration for is the modelled
//!    `ResourceNotFoundException` (HTTP 404); a value outside a member's modelled
//!    `@pattern`/`@length` is the modelled `ValidationException` (HTTP 400).
//! 4. "An IngestConfiguration that is active cannot be updated": while a publisher is
//!    broadcasting with it the configuration reports `ACTIVE`, an update is the modelled
//!    `ConflictException` (HTTP 409) and changes nothing; once that publisher has left,
//!    the same update succeeds.
//! 5. The answer carries the restriction headers the output shape binds, as every
//!    response of this endpoint does.
//!
//! Nothing in the IVS RealTime control plane makes a publisher BROADCAST to a stage: a
//! publisher reaches the stage over its data plane with the ingest key it was minted,
//! which is `state::join_stage_session` — the one place a participant record is created.
//! So the active cases bring publishers into a session through it, reaching the store
//! through this suite's own engine handle, exactly as the `DeleteIngestConfiguration`
//! suite beside this one does.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// The modelled `@length` maximum of `IngestConfigurationArn` and of
/// `IngestConfigurationStageArn`, from
/// `smithy_query(query="operation",operation="UpdateIngestConfiguration")`.
const ARN_MAX_LENGTH: usize = 128;

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`: a configuration is
/// `ACTIVE` while a publisher is publishing to its stage, and `INACTIVE` otherwise.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The `stageArn` of a configuration associated with no stage: the empty string the
/// modelled `IngestConfigurationStageArn` admits (the first alternative of its
/// `@pattern`, `^$`).
const NO_STAGE: &str = "";

/// The six restriction headers the modelled `UpdateIngestConfigurationResponse` binds,
/// by the `@httpHeader` name each carries
/// (`smithy_query(query="operation",operation="UpdateIngestConfiguration")`), keyed as an
/// HTTP client reads them.
const RESTRICTION_HEADERS: [&str; 6] = [
    "access-control-expose-headers",
    "cache-control",
    "content-security-policy",
    "strict-transport-security",
    "x-content-type-options",
    "x-frame-options",
];

/// One response, as a client sees it.
struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
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
    /// scope naming `ivs` in [`REGION`], and report the whole answer a client sees.
    async fn post(&self, operation: &str, body: &Value) -> Answer {
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
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    /// The status and body of a call, for the cases that assert nothing about headers.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let answer = self.post(operation, body).await;
        (answer.status, answer.body)
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

    /// `GetIngestConfiguration` for `arn`: the status and the body a client sees.
    async fn get(&self, arn: &str) -> (u16, Value) {
        self.call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await
    }

    /// The configuration `GetIngestConfiguration` reports for `arn`.
    async fn read(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.get(arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// `UpdateIngestConfiguration`, carrying the members the case varies: `stageArn` is
    /// absent when `None`, and `redundantIngest` absent when `None` so the modelled
    /// default applies.
    async fn update(
        &self,
        arn: &str,
        stage_arn: Option<&str>,
        redundant_ingest: Option<bool>,
    ) -> Answer {
        let mut request = json!({ "arn": arn });
        if let Some(stage_arn) = stage_arn {
            request["stageArn"] = json!(stage_arn);
        }
        if let Some(redundant_ingest) = redundant_ingest {
            request["redundantIngest"] = json!(redundant_ingest);
        }
        self.post("UpdateIngestConfiguration", &request).await
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

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// `configuration` as rule 1 says an update must leave it: `stageArn` replaced by
/// `stage_arn`, `redundantIngest` replaced by `redundant_ingest`, and the backup
/// credentials either the set it already carried or, where this update is what set the
/// switch, the set `answered` reports — whose SHAPE is asserted here, because a freshly
/// minted credential is a value no earlier read could carry.
fn after_update(
    case: &str,
    configuration: &Value,
    stage_arn: &str,
    redundant_ingest: bool,
    answered: &Value,
) -> Value {
    let mut expected = configuration.clone();
    expected["stageArn"] = json!(stage_arn);
    expected["redundantIngest"] = json!(redundant_ingest);
    match configuration.get("redundantIngestCredentials").cloned() {
        // Credentials the configuration already holds are its own: they are reported
        // whatever the switch now says.
        Some(carried) => expected["redundantIngestCredentials"] = carried,
        None if redundant_ingest => {
            let minted = answered
                .get("redundantIngestCredentials")
                .cloned()
                .unwrap_or(Value::Null);
            assert_minted_credentials(case, &minted, configuration);
            expected["redundantIngestCredentials"] = minted;
        }
        // A configuration whose switch was never set carries no list at all.
        None => {
            if let Some(members) = expected.as_object_mut() {
                members.remove("redundantIngestCredentials");
            }
        }
    }
    expected
}

/// The backup credentials an update that SET redundant ingest must report:
/// [`state::REDUNDANT_INGEST_CREDENTIALS`] of them, each carrying its own participant id
/// and its own ingest key of the modelled form, and none of them the primary credential of
/// `configuration`.
fn assert_minted_credentials(case: &str, minted: &Value, configuration: &Value) {
    let credentials = minted.as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected a redundantIngestCredentials list, got {minted}")
    });
    assert_eq!(
        credentials.len(),
        state::REDUNDANT_INGEST_CREDENTIALS,
        "case {case:?}: expected {} backup credentials, got {minted}",
        state::REDUNDANT_INGEST_CREDENTIALS
    );
    let primary_participant = member(configuration, "participantId");
    let primary_key = member(configuration, "streamKey");
    for credential in credentials {
        let participant_id = member(credential, "participantId");
        let stream_key = member(credential, "streamKey");
        assert!(
            !participant_id.is_empty() && participant_id != primary_participant,
            "case {case:?}: expected a backup participantId of its own, got {credential}"
        );
        assert!(
            stream_key.starts_with(STREAM_KEY_PREFIX) && stream_key != primary_key,
            "case {case:?}: expected a backup streamKey of its own starting \
             {STREAM_KEY_PREFIX:?}, got {credential}"
        );
    }
}

/// The first field of the modelled `StreamKey` `@pattern`
/// `^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$`, from
/// `smithy_query(query="shape",shape="StreamKey")` — the `ingest_configurations` suite
/// beside this one checks the whole pattern.
const STREAM_KEY_PREFIX: &str = "rt_";

/// How a row of the table below asks for the association, and the `stageArn` the
/// configuration must then report.
#[derive(Debug, Clone, Copy)]
enum Association {
    /// A stage this account holds, named outright.
    Stage,
    /// The other stage this account holds, so a move between two live stages is covered.
    OtherStage,
    /// The empty string the modelled member admits: no stage.
    Empty,
    /// No `stageArn` member at all, which is that same "no stage".
    Omitted,
}

impl Association {
    /// The member value the request carries, `None` when the row omits the member.
    fn requested<'a>(self, stage: &'a str, other_stage: &'a str) -> Option<&'a str> {
        match self {
            Self::Stage => Some(stage),
            Self::OtherStage => Some(other_stage),
            Self::Empty => Some(NO_STAGE),
            Self::Omitted => None,
        }
    }

    /// The `stageArn` the configuration reports after such an update.
    fn expected<'a>(self, stage: &'a str, other_stage: &'a str) -> &'a str {
        match self {
            Self::Stage => stage,
            Self::OtherStage => other_stage,
            Self::Empty | Self::Omitted => NO_STAGE,
        }
    }
}

/// The create requests this suite spans the class of updatable configurations with: each
/// row differs from the others in what rule 1 generalises over — the ingest protocol,
/// whether redundant ingest is on (so the record carries backup credentials too), which
/// optional members it carries, and whether it starts out associated with a stage.
///
/// `stage` is substituted into the row that starts out associated.
fn configuration_rows(stage: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest request the model allows",
            json!({ "ingestProtocol": "RTMPS" }),
        ),
        (
            "every optional member supplied, already associated with a stage",
            json!({
                "ingestProtocol": "RTMPS",
                "name": "studio-feed",
                "userId": "user-7",
                "attributes": { "seat": "left", "row": "front" },
                "tags": { "env": "test" },
                "stageArn": stage,
            }),
        ),
        (
            "cleartext RTMP ingest",
            json!({ "ingestProtocol": "RTMP", "insecureIngest": true, "name": "legacy" }),
        ),
        (
            "redundant ingest enabled, associated with a stage",
            json!({
                "ingestProtocol": "RTMPS",
                "redundantIngest": true,
                "stageArn": stage,
            }),
        ),
        (
            "an explicitly empty association",
            json!({ "ingestProtocol": "RTMPS", "stageArn": NO_STAGE }),
        ),
    ]
}

/// The `redundantIngest` values a request can carry, so every row below drives one of
/// them: the member is absent, explicitly `false`, or explicitly `true`.
const REQUESTED_REDUNDANT_INGEST: [Option<bool>; 3] = [None, Some(false), Some(true)];

// ---------------------------------------------------------------------------
// RULE 1 and 2: what an update writes, and what it leaves alone
// ---------------------------------------------------------------------------

/// An update writes the association the request asks for and nothing else: the answer is
/// the configuration as it was, with `stageArn` replaced; `GetIngestConfiguration`
/// reports that same configuration afterwards; and a repeat of the same update answers
/// the same thing again.
#[tokio::test]
async fn an_update_writes_the_association_and_leaves_every_other_member_alone() {
    let emulator = RealTime::new();
    let stage = emulator.create_stage("the fixture stage", "updates").await;
    let other_stage = emulator.create_stage("the fixture stage", "elsewhere").await;
    // A configuration no case updates, so every update is checked against a store that
    // also holds something it must not touch.
    let bystander = emulator
        .create(
            "the bystander",
            &json!({ "ingestProtocol": "RTMPS", "name": "keep", "stageArn": stage }),
        )
        .await;
    let bystander_arn = member(&bystander, "arn");

    for (row, request) in configuration_rows(&stage) {
        for (index, association) in [
            Association::Stage,
            Association::OtherStage,
            Association::Empty,
            Association::Omitted,
        ]
        .into_iter()
        .enumerate()
        {
            // Each row drives one of the `redundantIngest` values a request can carry,
            // so the table covers an absent member and both explicit ones.
            let redundant_ingest = REQUESTED_REDUNDANT_INGEST[index % REQUESTED_REDUNDANT_INGEST.len()];
            let case =
                format!("{row} moved to {association:?} with redundantIngest {redundant_ingest:?}");
            let created = emulator.create(&case, &request).await;
            let configuration_arn = member(&created, "arn");
            let before = emulator.read(&case, &configuration_arn).await;

            let requested = association.requested(&stage, &other_stage);
            let expected_stage = association.expected(&stage, &other_stage);
            // An absent `redundantIngest` carries the modelled default, `false`.
            let expected_redundant = redundant_ingest.unwrap_or(false);

            let answer = emulator
                .update(&configuration_arn, requested, redundant_ingest)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} {}",
                answer.status, answer.body
            );
            let expected = after_update(
                &case,
                &before,
                expected_stage,
                expected_redundant,
                &answer.body["ingestConfiguration"],
            );
            assert_eq!(
                answer.body["ingestConfiguration"], expected,
                "case {case:?}: expected {expected}, got {}",
                answer.body["ingestConfiguration"]
            );
            // Nobody publishes to an emulated stage here, so the reported state stays
            // the no-publisher one across the move.
            let state = member(&answer.body["ingestConfiguration"], "state");
            assert_eq!(
                state, NO_PUBLISHER_STATE,
                "case {case:?}: expected state {NO_PUBLISHER_STATE:?}, got {state:?}"
            );

            // The write round-trips: the reader reports what the update answered.
            let after = emulator.read(&case, &configuration_arn).await;
            assert_eq!(
                after, expected,
                "case {case:?}: expected GetIngestConfiguration to report {expected}, \
                 got {after}"
            );

            // Repeating it answers the same configuration again.
            let repeated = emulator
                .update(&configuration_arn, requested, redundant_ingest)
                .await;
            assert_eq!(
                repeated.status, 200,
                "case {case:?}: expected the repeated update to answer HTTP 200, got {} {}",
                repeated.status, repeated.body
            );
            assert_eq!(
                repeated.body["ingestConfiguration"], expected,
                "case {case:?}: expected the repeated update to answer {expected}, got {}",
                repeated.body["ingestConfiguration"]
            );
        }
    }

    // The configuration no case named was never touched by any of it.
    let still_there = emulator.read("the bystander", &bystander_arn).await;
    assert_eq!(
        still_there, bystander,
        "expected the bystander unchanged, expected {bystander}, got {still_there}"
    );
}

/// A `stageArn` naming a stage this account holds none of is the modelled
/// `ValidationException` (HTTP 400), stated the way every unresolved reference of this
/// service is, and the stored association is left as it was.
#[tokio::test]
async fn a_stage_this_account_does_not_hold_is_refused_and_persists_nothing() {
    let emulator = RealTime::new();
    let stage = emulator.create_stage("the fixture stage", "references").await;
    let deleted_stage = emulator.create_stage("the fixture stage", "gone").await;
    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": deleted_stage }))
        .await;
    assert_eq!(
        status, 200,
        "the fixture stage must delete, got {status} {response}"
    );

    // Each row is a well-formed `IngestConfigurationStageArn` no stage of this store
    // answers for — one never created, one in another region, one that existed and was
    // deleted, and one at the member's modelled length maximum.
    let cases = [
        ("a stage id nothing was created for", arn(REGION, "stage", "absent123456")),
        ("a stage of another region", arn("eu-west-1", "stage", "elsewhere123")),
        ("a stage that was deleted", deleted_stage.clone()),
        (
            "a stage id at the modelled length maximum",
            {
                let prefix = arn(REGION, "stage", "");
                format!("{prefix}{}", "a".repeat(ARN_MAX_LENGTH - prefix.len()))
            },
        ),
    ];

    for (case, stage_arn) in cases {
        assert!(
            stage_arn.len() <= ARN_MAX_LENGTH,
            "case {case:?}: the case's own ARN must be within the modelled length, \
             got {stage_arn:?}"
        );
        let configuration = emulator
            .create(
                case,
                &json!({ "ingestProtocol": "RTMPS", "stageArn": stage }),
            )
            .await;
        let configuration_arn = member(&configuration, "arn");

        let answer = emulator.update(&configuration_arn, Some(&stage_arn), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {stage_arn:?}, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.body
        );
        let expected = format!(
            "UpdateIngestConfiguration.stageArn is required to name a stage this account \
             holds, found {stage_arn}"
        );
        let message = member(&answer.body, "message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // Nothing was written: the configuration still names the stage it was created
        // with.
        let after = emulator.read(case, &configuration_arn).await;
        assert_eq!(
            after, configuration,
            "case {case:?}: expected the configuration unchanged, expected {configuration}, \
             got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: an ARN nothing answers for, and a value outside the model
// ---------------------------------------------------------------------------

/// An `arn` this store holds no configuration for is the modelled
/// `ResourceNotFoundException` (HTTP 404), stating the identifier the request named.
#[tokio::test]
async fn an_arn_no_configuration_answers_for_is_not_found() {
    let emulator = RealTime::new();
    let stage = emulator.create_stage("the fixture stage", "missing").await;
    // A configuration that was created and then deleted, so the not-found covers an ARN
    // this store once held as well as one it never did.
    let deleted = emulator
        .create("deleted", &json!({ "ingestProtocol": "RTMPS" }))
        .await;
    let deleted_arn = member(&deleted, "arn");
    let (status, response) = emulator
        .call("DeleteIngestConfiguration", &json!({ "arn": deleted_arn }))
        .await;
    assert_eq!(
        status, 200,
        "the fixture configuration must delete, got {status} {response}"
    );

    let cases = [
        (
            "an id nothing was created for",
            arn(REGION, "ingest-configuration", "absent123456"),
        ),
        (
            "a configuration of another region",
            arn("ap-south-1", "ingest-configuration", "elsewhere123"),
        ),
        ("a configuration that was deleted", deleted_arn.clone()),
    ];

    for (case, identifier) in cases {
        for requested in [None, Some(stage.as_str()), Some(NO_STAGE)] {
            let answer = emulator.update(&identifier, requested, None).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} stageArn={requested:?}: expected HTTP 404, got {} {}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer.body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            let expected = format!("Resource: {identifier} not found");
            let message = member(&answer.body, "message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// A value outside a member's modelled constraints is refused as a `ValidationException`
/// (HTTP 400) — not as a not-found, and not as a 501.
#[tokio::test]
async fn a_member_outside_the_modelled_constraints_is_a_validation_failure() {
    let emulator = RealTime::new();
    let configuration = emulator
        .create("the fixture configuration", &json!({ "ingestProtocol": "RTMPS" }))
        .await;
    let configuration_arn = member(&configuration, "arn");

    // `arn`: the empty string is under its `@length` minimum of 1, and each other row is
    // outside its `@pattern`.
    let arn_cases = [
        ("the empty string, under the @length minimum", String::new()),
        ("not an ARN at all", "ingest-configuration".to_string()),
        ("another resource type", arn(REGION, "stage", "abcdef123456")),
        (
            "an id carrying a character outside the pattern",
            arn(REGION, "ingest-configuration", "abc_def"),
        ),
        (
            "one character past the @length maximum",
            format!(
                "arn:aws:ivs:{REGION}:{ACCOUNT}:ingest-configuration/{}",
                "a".repeat(ARN_MAX_LENGTH)
            ),
        ),
    ];
    for (case, identifier) in arn_cases {
        let answer = emulator.update(&identifier, None, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {identifier:?}, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.body
        );
        let message = member(&answer.body, "message");
        assert!(
            message.starts_with("UpdateIngestConfiguration.arn "),
            "case {case:?}: expected a refusal naming the operation and the member, \
             got {message:?}"
        );
    }

    // `stageArn`: outside its `@pattern`, which admits the empty string and a stage ARN
    // and nothing else.
    let stage_cases = [
        ("not an ARN at all", "stage".to_string()),
        (
            "another resource type",
            arn(REGION, "ingest-configuration", "abcdef123456"),
        ),
        (
            "a stage id carrying a character outside the pattern",
            arn(REGION, "stage", "abc_def"),
        ),
        (
            "one character past the @length maximum",
            format!(
                "arn:aws:ivs:{REGION}:{ACCOUNT}:stage/{}",
                "a".repeat(ARN_MAX_LENGTH)
            ),
        ),
    ];
    for (case, stage_arn) in stage_cases {
        let answer = emulator
            .update(&configuration_arn, Some(&stage_arn), None)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {stage_arn:?}, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.body
        );
        let message = member(&answer.body, "message");
        assert!(
            message.starts_with("UpdateIngestConfiguration.stageArn "),
            "case {case:?}: expected a refusal naming the operation and the member, \
             got {message:?}"
        );
    }

    // A request that omits the `@required` member is refused the same way.
    let (status, response) = emulator
        .call("UpdateIngestConfiguration", &json!({}))
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
// RULE 4: a configuration a publisher is broadcasting with
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

/// While a publisher is broadcasting with a configuration it reports `ACTIVE`, an update
/// is the modelled `ConflictException` (HTTP 409) and writes nothing; once that publisher
/// has left the session, the same update succeeds. Exercised over every identity a
/// publisher can carry, and over both a move to another stage and a dissociation.
#[tokio::test]
async fn an_active_configuration_cannot_be_updated() {
    for identity in [
        PublisherIdentity::ConfigurationArn,
        PublisherIdentity::PrimaryCredential,
        PublisherIdentity::RedundantCredential,
    ] {
        for association in [Association::OtherStage, Association::Empty] {
            let case = format!("{identity:?} publishing, update to {association:?}");
            let emulator = RealTime::new();
            let stage = emulator.create_stage(&case, "active").await;
            let other_stage = emulator.create_stage(&case, "target").await;
            let configuration = emulator
                .create(
                    &case,
                    &json!({
                        "ingestProtocol": "RTMPS",
                        "redundantIngest": true,
                        "stageArn": stage,
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
                "case {case:?}: the publishing participant must be identified, \
                 got {configuration}"
            );
            emulator.join(
                &case,
                &stage,
                state::JoinedParticipant {
                    participant_id: participant_id.clone(),
                    published: true,
                    redundant_ingest: true,
                    ingest_configuration_arn: configuration_of_join,
                    ..Default::default()
                },
            );
            let session_id = emulator.active_session_id(&case, &stage).await;
            assert!(
                !session_id.is_empty(),
                "case {case:?}: the join must have opened a session of {stage}"
            );

            // The configuration is ACTIVE while that publisher is connected …
            let publishing = emulator.read(&case, &configuration_arn).await;
            let state = member(&publishing, "state");
            assert_eq!(
                state, PUBLISHING_STATE,
                "case {case:?}: expected state {PUBLISHING_STATE:?} while a publisher is \
                 broadcasting, got {state:?}"
            );

            // … so the update is refused, naming what is in use, and writes nothing.
            let requested = association.requested(&stage, &other_stage);
            let answer = emulator.update(&configuration_arn, requested, None).await;
            assert_eq!(
                answer.status, 409,
                "case {case:?}: expected HTTP 409 while publishing, got {} {}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer.body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                answer.body
            );
            let expected = format!(
                "Resource: {configuration_arn} is in use: participant {participant_id} is \
                 publishing with it to {stage}"
            );
            let message = member(&answer.body, "message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            let after_refusal = emulator.read(&case, &configuration_arn).await;
            assert_eq!(
                after_refusal, publishing,
                "case {case:?}: expected the configuration unchanged by the refusal, \
                 expected {publishing}, got {after_refusal}"
            );

            // Once the publisher has left, the same update succeeds.
            emulator.leave(&case, &stage, &session_id, &participant_id);
            let answer = emulator.update(&configuration_arn, requested, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 once nobody publishes, got {} {}",
                answer.status, answer.body
            );
            let expected_stage = association.expected(&stage, &other_stage);
            let reported = member(&answer.body["ingestConfiguration"], "stageArn");
            assert_eq!(
                reported, expected_stage,
                "case {case:?}: expected stageArn {expected_stage:?}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 5: the headers the output shape binds
// ---------------------------------------------------------------------------

/// The answer carries every restriction header the modelled response binds, with the same
/// value the rest of this endpoint's answers carry for it.
#[tokio::test]
async fn the_answer_carries_the_restriction_headers_the_response_binds() {
    let emulator = RealTime::new();
    let stage = emulator.create_stage("the fixture stage", "headers").await;
    let created = emulator
        .post(
            "CreateIngestConfiguration",
            &json!({ "ingestProtocol": "RTMPS" }),
        )
        .await;
    let configuration_arn = member(&created.body["ingestConfiguration"], "arn");

    let answer = emulator
        .update(&configuration_arn, Some(&stage), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "the update must answer HTTP 200, got {} {}",
        answer.status, answer.body
    );
    for header in RESTRICTION_HEADERS {
        let updated = answer.headers.get(header).map(String::as_str);
        let on_create = created.headers.get(header).map(String::as_str);
        assert_eq!(
            updated, on_create,
            "case {header:?}: expected the value this endpoint answers with \
             ({on_create:?}), got {updated:?}"
        );
        assert!(
            updated.is_some_and(|value| !value.is_empty()),
            "case {header:?}: expected a value, got {updated:?}"
        );
    }
}
