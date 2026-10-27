// SPDX-License-Identifier: CC0-1.0

//! The identity every creating operation of this service mints.
//!
//! RULE: a create draws a fresh id from the store's monotonic sequence at the length its
//! resource's modelled id allows, re-draws while the identity that id names is already
//! held, and reports it as the resource's identity — so no two resources this store holds
//! share an ARN, and the matching `Get…` reports the resource back for the ARN the create
//! minted.
//!
//! The table below is one row per resource this service mints — `CreateStage`,
//! `CreateIngestConfiguration`, `CreateEncoderConfiguration`,
//! `CreateStorageConfiguration` and `StartComposition` — so the rows differ in the map
//! the resource is held in, the ARN form it carries, the id length it allows and the read
//! that reports it back. The participant ids `CreateStage` and `CreateParticipantToken`
//! issue are the same rule over an identity that is not an ARN, which the second test
//! pins.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status and the JSON body.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region a call that does not vary it is signed for.
const REGION: &str = "us-east-1";

/// The modelled `@length` maximum every ARN of this service shares: `StageArn`,
/// `IngestConfigurationArn`, `EncoderConfigurationArn`, `StorageConfigurationArn` and
/// `CompositionArn` are each `@length(1..128)`
/// (`smithy_query(query="shape",shape="StageArn,IngestConfigurationArn,\
/// EncoderConfigurationArn,StorageConfigurationArn,CompositionArn")`).
const ARN_MAX_LENGTH: usize = 128;

/// How many resources of each kind one row mints, so uniqueness is asserted over a set
/// rather than over a single pair.
const MINTS_PER_ROW: usize = 4;

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

    /// An operation bound to `POST /<Operation>`, signed for `region`.
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

    /// Call `operation` and assert the modelled success, answering with the body.
    async fn succeed(&self, case: &str, region: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        response
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

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The resources a composition and an ingest configuration refer to, created in the
/// region the row runs in.
struct Referenced {
    stage_arn: String,
}

impl Referenced {
    async fn resolve(emulator: &RealTime, case: &str, region: &str) -> Self {
        let created = emulator
            .succeed(case, region, "CreateStage", &json!({ "name": "referenced" }))
            .await;
        Self {
            stage_arn: member(&created["stage"], "arn"),
        }
    }
}

/// One row of the mint class: the creating operation, where its response reports the
/// resource, the ARN resource type it mints, the id length its model allows, and the read
/// that must report the resource back for the minted ARN.
struct Minting {
    create: &'static str,
    created_member: &'static str,
    resource_type: &'static str,
    id_len: usize,
    read: &'static str,
    read_member: &'static str,
}

impl Minting {
    /// The request this row sends, resolved against the resources of its own region.
    fn request(&self, referenced: &Referenced, region: &str) -> Value {
        match self.create {
            "CreateStage" => json!({ "name": "minted" }),
            "CreateIngestConfiguration" => json!({
                "ingestProtocol": "RTMPS",
                "stageArn": referenced.stage_arn,
            }),
            "CreateEncoderConfiguration" => json!({ "name": "minted" }),
            "CreateStorageConfiguration" => json!({
                "s3": { "bucketName": "minted-recordings" },
            }),
            "StartComposition" => json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": arn(region, "channel", "live") } }],
            }),
            other => panic!("this suite has no request for {other}"),
        }
    }
}

/// Every resource this service mints, one row each. The id lengths are the crate's own
/// declared lengths, each documented against the modelled id pattern it has to satisfy.
fn mint_rows() -> Vec<Minting> {
    vec![
        Minting {
            create: "CreateStage",
            created_member: "stage",
            resource_type: "stage",
            id_len: state::STAGE_ID_LEN,
            read: "GetStage",
            read_member: "stage",
        },
        Minting {
            create: "CreateIngestConfiguration",
            created_member: "ingestConfiguration",
            resource_type: "ingest-configuration",
            id_len: state::INGEST_CONFIGURATION_ID_LEN,
            read: "GetIngestConfiguration",
            read_member: "ingestConfiguration",
        },
        Minting {
            create: "CreateEncoderConfiguration",
            created_member: "encoderConfiguration",
            resource_type: "encoder-configuration",
            id_len: state::ENCODER_CONFIGURATION_ID_LEN,
            read: "GetEncoderConfiguration",
            read_member: "encoderConfiguration",
        },
        Minting {
            create: "CreateStorageConfiguration",
            created_member: "storageConfiguration",
            resource_type: "storage-configuration",
            id_len: state::STORAGE_CONFIGURATION_ID_LEN,
            read: "GetStorageConfiguration",
            read_member: "storageConfiguration",
        },
        Minting {
            create: "StartComposition",
            created_member: "composition",
            resource_type: "composition",
            id_len: state::COMPOSITION_ID_LEN,
            read: "GetComposition",
            read_member: "composition",
        },
    ]
}

/// The id an ARN of `resource_type` in `region` names, or `None` when the ARN is not of
/// the form the modelled `@pattern` requires: `arn:aws:ivs:<region>:<account>:<type>/<id>`
/// with the id drawn from `[a-zA-Z0-9-]`.
fn minted_id<'a>(arn: &'a str, region: &str, resource_type: &str) -> Option<&'a str> {
    let prefix = format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/");
    let id = arn.strip_prefix(&prefix)?;
    let id_like = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    id_like.then_some(id)
}

// ---------------------------------------------------------------------------
// The rule over every creating operation
// ---------------------------------------------------------------------------

/// Every create mints an identity of its resource's own modelled form, unique across
/// everything this store holds, and the matching read reports the resource back for it.
#[tokio::test]
async fn every_create_mints_a_unique_identity_the_matching_read_reports_back() {
    // Two regions, so the rule is pinned where the ARN a row mints carries the region the
    // request was signed for rather than one fixed region.
    for region in [REGION, "ap-northeast-3"] {
        let emulator = RealTime::new();
        let referenced = Referenced::resolve(&emulator, "referenced resources", region).await;

        // Every ARN this store has minted, across kinds: one ARN may never denote two
        // resources, whichever operations minted them.
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(referenced.stage_arn.clone());

        for row in mint_rows() {
            for attempt in 1..=MINTS_PER_ROW {
                let case = format!("{} in {region} (mint {attempt})", row.create);
                let request = row.request(&referenced, region);
                let created = emulator
                    .succeed(&case, region, row.create, &request)
                    .await[row.created_member]
                    .clone();
                let minted = member(&created, "arn");

                let id = minted_id(&minted, region, row.resource_type);
                assert!(
                    id.is_some(),
                    "case {case:?}: expected an ARN of the form \
                     arn:aws:ivs:{region}:{ACCOUNT}:{}/<id>, got {minted:?}",
                    row.resource_type
                );
                let id = id.unwrap_or_default();
                assert_eq!(
                    id.chars().count(),
                    row.id_len,
                    "case {case:?}: expected a {}-character id, got {id:?}",
                    row.id_len
                );
                assert!(
                    minted.len() <= ARN_MAX_LENGTH,
                    "case {case:?}: expected an ARN within the modelled {ARN_MAX_LENGTH} \
                     characters, got {} in {minted:?}",
                    minted.len()
                );
                assert!(
                    seen.insert(minted.clone()),
                    "case {case:?}: expected an ARN no resource of this store already \
                     carries, got the repeat {minted:?}"
                );

                let read = emulator
                    .succeed(&case, region, row.read, &json!({ "arn": minted }))
                    .await[row.read_member]
                    .clone();
                let reported = member(&read, "arn");
                assert_eq!(
                    reported, minted,
                    "case {case:?}: {} must report the minted resource, expected \
                     {minted:?}, got {reported:?} in {read}",
                    row.read
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The same rule over an identity that is not an ARN
// ---------------------------------------------------------------------------

/// A participant id is minted by the same rule: every token this store issues for a stage
/// carries an id of the modelled length, distinct from every other id of that stage —
/// whether the tokens were issued together by one `CreateStage` or one at a time by
/// `CreateParticipantToken`.
#[tokio::test]
async fn every_participant_token_carries_an_id_distinct_from_the_others_of_its_stage() {
    let emulator = RealTime::new();

    // The rows differ in how many tokens ONE request asks for: the smallest request that
    // asks for any, a middling one, and the modelled maximum —
    // `CreateStageRequest$participantTokenConfigurations` is `@length(0..12)`
    // (`smithy_query(query="operation",operation="CreateStage")`).
    for requested in [1_usize, 3, 12] {
        let case = format!("a stage created with {requested} token configurations");
        let configurations: Vec<Value> = (0..requested)
            .map(|index| json!({ "userId": format!("user-{index}") }))
            .collect();
        let created = emulator
            .succeed(
                &case,
                REGION,
                "CreateStage",
                &json!({ "participantTokenConfigurations": configurations }),
            )
            .await;
        let stage_arn = member(&created["stage"], "arn");
        let tokens = created["participantTokens"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tokens.len(),
            requested,
            "case {case:?}: expected {requested} participant tokens, got {}",
            tokens.len()
        );

        let mut ids: HashSet<String> = HashSet::new();
        for token in &tokens {
            let id = member(token, "participantId");
            assert_eq!(
                id.chars().count(),
                state::PARTICIPANT_ID_LEN,
                "case {case:?}: expected a {}-character participant id, got {id:?}",
                state::PARTICIPANT_ID_LEN
            );
            assert!(
                ids.insert(id.clone()),
                "case {case:?}: expected an id no other token of this stage carries, \
                 got the repeat {id:?}"
            );
        }

        // Tokens issued one at a time against that same stage keep the rule: the id is
        // checked against the ones already stored, not only against this request's own.
        for index in 0..3 {
            let case = format!("{case} / CreateParticipantToken {index}");
            let issued = emulator
                .succeed(
                    &case,
                    REGION,
                    "CreateParticipantToken",
                    &json!({ "stageArn": stage_arn, "userId": format!("later-{index}") }),
                )
                .await;
            let id = member(&issued["participantToken"], "participantId");
            assert_eq!(
                id.chars().count(),
                state::PARTICIPANT_ID_LEN,
                "case {case:?}: expected a {}-character participant id, got {id:?}",
                state::PARTICIPANT_ID_LEN
            );
            assert!(
                ids.insert(id.clone()),
                "case {case:?}: expected an id no token of this stage already carries, \
                 got the repeat {id:?}"
            );
        }
    }
}
