// SPDX-License-Identifier: CC0-1.0

//! Redundant ingest as a SWITCH on an ingest configuration: what the operations that set
//! it leave behind, and what every reader of the configuration then reports.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! signed `aws ivs-realtime` calls arrive, so what is asserted is the emulator's HTTP
//! behaviour and never a handler called directly.
//!
//! The RULE under test, exercised over a SET of inputs rather than on one request: an
//! operation whose input carries `redundantIngest` leaves the configuration in the state
//! that member names (an absent member carrying the modelled default `false`), and SETTING
//! that switch is what materialises the backup credentials:
//! `IngestConfiguration$redundantIngestCredentials` is "a list of redundant ingest
//! credentials, present only when `redundantIngest` is set to `true`", one credential per
//! redundant ingest path, each with its own `participantId` and its own `streamKey`. Those
//! credentials are the configuration's from then on — a configuration whose switch has
//! since gone off still reports the ingest keys it handed out, and one whose switch was
//! never set reports no such member at all — and every projection of the configuration (the
//! answer of the call that wrote it, `GetIngestConfiguration`, and the
//! `ListIngestConfigurations` summary, which carries `redundantIngest` but no credentials)
//! reports that same state.
//!
//! The rows differ in what the rule generalises over: the state the configuration was
//! created in, the state the update asks for (absent, `false`, `true`), how the request
//! names its stage association, and the ingest protocol — so setting the switch, clearing
//! it, leaving it as it stands and setting it again are each covered, on configurations
//! that start from either state.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// The first field of the modelled `StreamKey` `@pattern`
/// `^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$`
/// (`smithy_query(query="shape",shape="StreamKey")`): an ingest key names the real-time
/// ingest it belongs to before anything else. The `ingest_configurations` suite checks the
/// whole pattern; what matters here is that each credential carries a key of that form and
/// its own id.
const STREAM_KEY_PREFIX: &str = "rt_";

/// The `stageArn` of a configuration associated with no stage: the empty string the
/// modelled `IngestConfigurationStageArn` admits (the first alternative of its `@pattern`,
/// `^$`).
const NO_STAGE: &str = "";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`], and report the status and body a client sees.
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
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, body)
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

    /// Create an ingest configuration and return the `ingestConfiguration` reported for it.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// `UpdateIngestConfiguration`, carrying `redundantIngest` only when the row names a
    /// value for it and `stageArn` only when it names an association.
    async fn update(
        &self,
        case: &str,
        arn: &str,
        stage_arn: Option<&str>,
        redundant_ingest: Option<bool>,
    ) -> Value {
        let mut request = json!({ "arn": arn });
        if let Some(stage_arn) = stage_arn {
            request["stageArn"] = json!(stage_arn);
        }
        if let Some(redundant_ingest) = redundant_ingest {
            request["redundantIngest"] = json!(redundant_ingest);
        }
        let (status, response) = self.call("UpdateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// The configuration `GetIngestConfiguration` reports for `arn`.
    async fn read(&self, case: &str, arn: &str) -> Value {
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

    /// The `ListIngestConfigurations` summary of `arn`.
    async fn summary(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("ListIngestConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListIngestConfigurations must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a page of summaries, got {response}"))
            .iter()
            .find(|summary| member(summary, "arn") == arn)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn} in the page, got {response}"))
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `redundantIngestCredentials` of a reported configuration, or `None` when the member
/// is absent — which is how the model says "none".
fn credentials(configuration: &Value) -> Option<Vec<Value>> {
    configuration
        .get("redundantIngestCredentials")
        .map(|member| {
            member
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("redundantIngestCredentials must be a list, got {member}"))
        })
}

/// Assert the pair `redundantIngest`/`redundantIngestCredentials` of `configuration`: the
/// switch reports `expected_switch`, the credentials are present exactly when
/// `expected_credentials` says the configuration has had the switch set, there are
/// [`state::REDUNDANT_INGEST_CREDENTIALS`] of them, and each carries its own participant id
/// and its own ingest key — none of them the primary credential the configuration publishes
/// with itself.
fn assert_redundant_ingest(
    case: &str,
    configuration: &Value,
    expected_switch: bool,
    expected_credentials: bool,
) {
    let reported = configuration
        .get("redundantIngest")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a redundantIngest, got {configuration}"));
    assert_eq!(
        reported, expected_switch,
        "case {case:?}: expected redundantIngest {expected_switch}, got {configuration}"
    );
    let Some(credentials) = credentials(configuration) else {
        assert!(
            !expected_credentials,
            "case {case:?}: expected the backup credentials the switch materialised, \
             got {configuration}"
        );
        return;
    };
    assert!(
        expected_credentials,
        "case {case:?}: expected no redundantIngestCredentials for a configuration whose \
         switch was never set, got {configuration}"
    );
    assert_eq!(
        credentials.len(),
        state::REDUNDANT_INGEST_CREDENTIALS,
        "case {case:?}: expected {} backup credentials, got {configuration}",
        state::REDUNDANT_INGEST_CREDENTIALS
    );
    let mut seen = HashSet::from([
        member(configuration, "participantId"),
        member(configuration, "streamKey"),
    ]);
    for credential in &credentials {
        let participant_id = member(credential, "participantId");
        let stream_key = member(credential, "streamKey");
        assert!(
            !participant_id.is_empty() && participant_id.len() <= 64,
            "case {case:?}: expected a backup participantId inside the modelled length, \
             got {credential}"
        );
        assert!(
            participant_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "case {case:?}: expected a backup participantId inside the modelled pattern \
             ^[a-zA-Z0-9-_]*$, got {credential}"
        );
        assert!(
            stream_key.starts_with(STREAM_KEY_PREFIX),
            "case {case:?}: expected a backup streamKey starting {STREAM_KEY_PREFIX:?}, \
             got {credential}"
        );
        assert!(
            seen.insert(participant_id.clone()),
            "case {case:?}: expected a backup participantId of its own, got the \
             already-issued {participant_id:?} ({configuration})"
        );
        assert!(
            seen.insert(stream_key.clone()),
            "case {case:?}: expected a backup streamKey of its own, got an already-issued \
             key ({configuration})"
        );
    }
}

/// Every reader of the configuration reports the same state the writing call answered: the
/// full read member for member, and the listing's summary for the members a summary
/// carries — `redundantIngest` among them, the credentials not
/// (`smithy_query(query="shape",shape="IngestConfigurationSummary")`).
async fn assert_readers_agree(emulator: &RealTime, case: &str, written: &Value) {
    let arn = member(written, "arn");
    let read = emulator.read(case, &arn).await;
    assert_eq!(
        &read, written,
        "case {case:?}: expected GetIngestConfiguration to report {written}, got {read}"
    );
    let summary = emulator.summary(case, &arn).await;
    assert_eq!(
        summary.get("redundantIngest"),
        written.get("redundantIngest"),
        "case {case:?}: expected the summary to report the same redundantIngest as \
         {written}, got {summary}"
    );
    assert!(
        credentials(&summary).is_none(),
        "case {case:?}: the summary shape carries no credentials, got {summary}"
    );
}

/// How a row asks for the stage association of its update.
#[derive(Debug, Clone, Copy)]
enum Association {
    /// A stage this account holds, named outright.
    Stage,
    /// The empty string the modelled member admits: no stage.
    Empty,
    /// No `stageArn` member at all, which is that same "no stage".
    Omitted,
}

impl Association {
    /// The member value the request carries, `None` where the row omits the member.
    fn requested(self, stage: &str) -> Option<&str> {
        match self {
            Self::Stage => Some(stage),
            Self::Empty => Some(NO_STAGE),
            Self::Omitted => None,
        }
    }
}

/// One row of the toggle table: the configuration to create, the state the update asks
/// for, and how it names the association.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// What `CreateIngestConfiguration` is called with, beside the stage substitution.
    created: Value,
    /// The `redundantIngest` the creating request carried, so the state before the update
    /// is known to the row rather than read back from the service.
    created_redundant: bool,
    /// The `redundantIngest` the update carries, `None` where it carries the member not at
    /// all — the modelled default, `false`.
    requested: Option<bool>,
    association: Association,
}

/// The rows: every combination of "the state it was created in" × "the state the update
/// asks for" that the member admits, over both ingest protocols and each way of naming the
/// association.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "off, switched on",
            created: json!({ "ingestProtocol": "RTMPS" }),
            created_redundant: false,
            requested: Some(true),
            association: Association::Stage,
        },
        Row {
            case: "off, left off by an absent member",
            created: json!({ "ingestProtocol": "RTMPS", "name": "left-alone" }),
            created_redundant: false,
            requested: None,
            association: Association::Omitted,
        },
        Row {
            case: "off, asked for off",
            created: json!({ "ingestProtocol": "RTMP", "insecureIngest": true }),
            created_redundant: false,
            requested: Some(false),
            association: Association::Empty,
        },
        Row {
            case: "on, switched off",
            created: json!({ "ingestProtocol": "RTMPS", "redundantIngest": true }),
            created_redundant: true,
            requested: Some(false),
            association: Association::Stage,
        },
        Row {
            case: "on, asked for on",
            created: json!({
                "ingestProtocol": "RTMPS",
                "redundantIngest": true,
                "userId": "user-7",
                "attributes": { "seat": "left" },
            }),
            created_redundant: true,
            requested: Some(true),
            association: Association::Empty,
        },
        Row {
            case: "on, switched off by an absent member",
            created: json!({
                "ingestProtocol": "RTMP",
                "insecureIngest": true,
                "redundantIngest": true,
            }),
            created_redundant: true,
            requested: None,
            association: Association::Omitted,
        },
        Row {
            case: "off, switched on while the configuration keeps its stage",
            created: json!({ "ingestProtocol": "RTMPS", "tags": { "env": "test" } }),
            created_redundant: false,
            requested: Some(true),
            association: Association::Omitted,
        },
    ]
}

/// A create reports the state its own `redundantIngest` named, and an update leaves the
/// configuration in the state ITS `redundantIngest` names, materialising the backup
/// credentials the first time that switch is set and keeping them afterwards, which every
/// reader then reports.
#[tokio::test]
async fn an_update_leaves_redundant_ingest_in_the_state_the_request_names() {
    let emulator = RealTime::new();
    let stage = emulator.create_stage("the fixture stage", "redundancy").await;

    for row in rows() {
        let case = row.case;
        let mut request = row.created.clone();
        if matches!(row.association, Association::Stage) {
            // A row that moves the configuration ONTO a stage starts off one, so the
            // association really changes with the switch.
            request["stageArn"] = json!(NO_STAGE);
        }
        let created = emulator.create(case, &request).await;
        // The create is the other operation that sets the switch: it reports the state its
        // own request named, with the credentials setting that switch materialises.
        assert_redundant_ingest(case, &created, row.created_redundant, row.created_redundant);
        assert_readers_agree(&emulator, case, &created).await;

        let arn = member(&created, "arn");
        let expected = row.requested.unwrap_or(false);
        // The configuration has had the switch set once either call set it, and the
        // credentials that minted stay with it.
        let has_credentials = row.created_redundant || expected;
        let updated = emulator
            .update(case, &arn, row.association.requested(&stage), row.requested)
            .await;
        assert_redundant_ingest(case, &updated, expected, has_credentials);
        assert_readers_agree(&emulator, case, &updated).await;

        // A set the configuration already carried is KEPT, whatever the switch now says, so
        // a publisher that cached a backup key keeps working.
        if row.created_redundant {
            assert_eq!(
                credentials(&updated),
                credentials(&created),
                "case {case:?}: expected the backup credentials kept across the update, \
                 got {updated}"
            );
        }

        // Repeating the same update answers the same configuration: nothing is re-minted.
        let repeated = emulator
            .update(case, &arn, row.association.requested(&stage), row.requested)
            .await;
        assert_eq!(
            repeated, updated,
            "case {case:?}: expected the repeated update to answer {updated}, got {repeated}"
        );

        // And setting the switch on a configuration that is off either mints the set it
        // never had, or reports the set it already holds.
        if !expected {
            let set_again = emulator
                .update(case, &arn, row.association.requested(&stage), Some(true))
                .await;
            assert_redundant_ingest(case, &set_again, true, true);
            assert_readers_agree(&emulator, case, &set_again).await;
            if let Some(before) = credentials(&updated) {
                assert_eq!(
                    credentials(&set_again),
                    Some(before),
                    "case {case:?}: expected the credentials it already held, got {set_again}"
                );
            }
        }
    }
}
