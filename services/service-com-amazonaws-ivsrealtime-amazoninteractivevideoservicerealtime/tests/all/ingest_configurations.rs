// SPDX-License-Identifier: CC0-1.0

//! The ingest-configuration surface over the wire: `CreateIngestConfiguration` and
//! `GetIngestConfiguration`.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. ROUND TRIP. `CreateIngestConfiguration` persists the configuration the request
//!    describes and mints what the request does not supply (the ARN, the participant id,
//!    the ingest key, the state), and `GetIngestConfiguration` reports that SAME
//!    configuration back for its ARN — member for member, on every read.
//! 2. MINTED VALUES take the modelled forms: the ARN the `IngestConfigurationArn`
//!    pattern, the ingest key the `StreamKey` pattern, the participant id the
//!    `ParticipantId` pattern; each is distinct between configurations, carries the
//!    region the creating request was signed for, and never changes once minted.
//! 3. A READ CHANGES NOTHING: `GetIngestConfiguration` is `@readonly`, so repeated reads
//!    answer identically and leave the configuration where it was.
//! 4. An ARN this store holds no configuration for is the modelled
//!    `ResourceNotFoundException` (HTTP 404); a value outside the member's modelled
//!    `@pattern`/`@length` is the modelled `ValidationException` (HTTP 400).
//! 5. The two DOCUMENTED rules the model carries no trait for are enforced: cleartext
//!    `RTMP` ingest requires `insecureIngest`, and an `attributes` map may reach 1 KB.
//! 6. A REFERENCE is resolved against state: the stage a create associates the
//!    configuration with has to be a stage this account holds, so a well-formed stage ARN
//!    no stage answers for is refused (HTTP 400 `ValidationException`) and persists
//!    nothing, while the empty string — the modelled "no stage" — is accepted.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// `IngestConfigurationArn`'s `@pattern`, from
/// `smithy_query(query="shape",shape="IngestConfiguration")` — member `arn`.
const INGEST_CONFIGURATION_ARN_PATTERN: &str =
    "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ingest-configuration/[a-zA-Z0-9-]+$";

/// `StreamKey`'s `@pattern`, from `smithy_query(query="shape",shape="StreamKey")`.
const STREAM_KEY_PATTERN: &str = "^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$";

/// `ParticipantId`'s `@pattern`, from
/// `smithy_query(query="shape",shape="IngestConfiguration")` — member `participantId`.
const PARTICIPANT_ID_PATTERN: &str = "^[a-zA-Z0-9-_]*$";

/// The modelled `@length` maximum of `IngestConfigurationArn`.
const INGEST_CONFIGURATION_ARN_MAX_LENGTH: usize = 128;

/// The state of a configuration nobody is publishing to, from the model's doc for
/// `IngestConfiguration$state` ("It is `ACTIVE` if a publisher currently is publishing
/// to the stage associated with the ingest configuration"). Nothing publishes to an
/// emulated stage.
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The documented maximum size of an `attributes` map, from
/// `CreateIngestConfigurationRequest$attributes`: "The maximum length of this field is
/// 1 KB total."
const MAX_ATTRIBUTES_BYTES: usize = 1024;

/// The values `IngestProtocol` accepts, from
/// `smithy_query(query="shape",shape="IngestProtocol")`.
const INGEST_PROTOCOLS: [&str; 2] = ["RTMP", "RTMPS"];

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
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] against the region every test that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, body: Option<Value>) -> (u16, Value) {
        let path = format!("/tags/{}", encode_path_label(arn));
        self.send(
            "us-east-1",
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
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

    /// Create an ingest configuration in `region` and return the `ingestConfiguration`
    /// the service reported for it.
    async fn create_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, "CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Read `arn` back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetIngestConfiguration", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
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

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Does `value` satisfy one of the three model `@pattern`s this suite checks?
///
/// The patterns are quoted on the constants above, straight from `smithy_query`, and are
/// simple enough to check directly — which keeps this crate's dependency set exactly what
/// the service itself needs. Each arm names the pattern it implements; a pattern with no
/// arm panics rather than passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    let region_like =
        |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let id_like =
        |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    match pattern {
        // ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ingest-configuration/[a-zA-Z0-9-]+$
        INGEST_CONFIGURATION_ARN_PATTERN => {
            let Some(rest) = value.strip_prefix("arn:aws:ivs:") else {
                return false;
            };
            let Some((region, rest)) = rest.split_once(':') else {
                return false;
            };
            let Some((account, resource)) = rest.split_once(':') else {
                return false;
            };
            let Some(id) = resource.strip_prefix("ingest-configuration/") else {
                return false;
            };
            region_like(region)
                && !account.is_empty()
                && account.bytes().all(|b| b.is_ascii_digit())
                && id_like(id)
        }
        // ^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$
        STREAM_KEY_PATTERN => {
            let Some(rest) = value.strip_prefix("rt_") else {
                return false;
            };
            let fields: Vec<&str> = rest.splitn(4, '_').collect();
            let [account, region, participant, secret] = fields.as_slice() else {
                return false;
            };
            !account.is_empty()
                && account.bytes().all(|b| b.is_ascii_digit())
                && region_like(region)
                && id_like(participant)
                && !secret.is_empty()
        }
        // ^[a-zA-Z0-9-_]*$
        PARTICIPANT_ID_PATTERN => value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        other => panic!("this suite has no checker for the pattern {other:?}"),
    }
}

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// An `attributes` map whose JSON encoding is `bytes` bytes — the measure the documented
/// 1 KB limit is stated in. One entry, so its punctuation is the two braces, the two
/// quotes around the key, the colon and the two quotes around the value.
fn attributes_of_size(bytes: usize) -> HashMap<String, String> {
    HashMap::from([("a".to_string(), "v".repeat(bytes - "{\"a\":\"\"}".len()))])
}

/// The create requests this suite spans the class of inputs with: each row differs from
/// the others in what the rule generalises over — the protocol, whether a stage is named,
/// whether redundant ingest is on, and which optional members the request carries.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest request the model allows",
            json!({ "ingestProtocol": "RTMPS" }),
        ),
        (
            "every optional member supplied",
            json!({
                "ingestProtocol": "RTMPS",
                "name": "studio-feed",
                "userId": "user-7",
                "attributes": { "seat": "left", "role": "host" },
                "redundantIngest": false,
                "tags": { "env": "test" },
            }),
        ),
        (
            "cleartext RTMP with insecure ingest allowed",
            json!({ "ingestProtocol": "RTMP", "insecureIngest": true, "name": "legacy" }),
        ),
        (
            "redundant ingest enabled",
            json!({ "ingestProtocol": "RTMPS", "redundantIngest": true }),
        ),
        (
            "an explicitly empty stage association",
            json!({ "ingestProtocol": "RTMPS", "stageArn": "" }),
        ),
        (
            "an attributes map at the documented 1 KB maximum",
            json!({
                "ingestProtocol": "RTMPS",
                "attributes": attributes_of_size(MAX_ATTRIBUTES_BYTES),
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// CreateIngestConfiguration, then GetIngestConfiguration
// ---------------------------------------------------------------------------

/// What a create persists is what a read reports: for every row of the input class, the
/// configuration `GetIngestConfiguration` answers is the one `CreateIngestConfiguration`
/// reported, member for member, and every member is the one the request asked for or the
/// modelled value for what it left out.
#[tokio::test]
async fn a_created_ingest_configuration_is_read_back_exactly() {
    let emulator = RealTime::new();
    // One stage, so a row that associates a configuration with a real stage names a
    // stage this store actually holds.
    let (status, created_stage) = emulator.call("CreateStage", &json!({ "name": "s" })).await;
    assert_eq!(status, 200, "CreateStage must answer HTTP 200, got {created_stage}");
    let stage_arn = member(&created_stage["stage"], "arn");

    let mut cases = create_cases();
    cases.push((
        "associated with an existing stage",
        json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn }),
    ));

    for (case, request) in cases {
        let created = emulator.create_in(case, "us-east-1", &request).await;
        let configuration_arn = member(&created, "arn");
        let read = emulator.get(case, &configuration_arn).await;

        assert_eq!(
            read, created,
            "case {case:?}: GetIngestConfiguration must report the created configuration, \
             expected {created}, got {read}"
        );

        // The minted ARN: the modelled form, within the modelled length, naming the
        // region the creating request was signed for.
        assert!(
            matches(INGEST_CONFIGURATION_ARN_PATTERN, &configuration_arn),
            "case {case:?}: expected an arn matching {INGEST_CONFIGURATION_ARN_PATTERN}, \
             got {configuration_arn:?}"
        );
        assert!(
            configuration_arn.len() <= INGEST_CONFIGURATION_ARN_MAX_LENGTH,
            "case {case:?}: expected an arn of at most {INGEST_CONFIGURATION_ARN_MAX_LENGTH} \
             characters, got {} in {configuration_arn:?}",
            configuration_arn.len()
        );

        // The minted ingest key and participant id.
        let stream_key = member(&read, "streamKey");
        assert!(
            matches(STREAM_KEY_PATTERN, &stream_key),
            "case {case:?}: expected a streamKey matching {STREAM_KEY_PATTERN}, got {stream_key:?}"
        );
        let participant_id = member(&read, "participantId");
        assert!(
            !participant_id.is_empty() && matches(PARTICIPANT_ID_PATTERN, &participant_id),
            "case {case:?}: expected a participantId matching {PARTICIPANT_ID_PATTERN}, \
             got {participant_id:?}"
        );
        assert!(
            stream_key.contains(&participant_id),
            "case {case:?}: expected the streamKey to name the configuration's participant \
             {participant_id:?}, got {stream_key:?}"
        );

        // What the request asked for, reported back.
        let expected_protocol = member(&request, "ingestProtocol");
        let actual_protocol = member(&read, "ingestProtocol");
        assert_eq!(
            actual_protocol, expected_protocol,
            "case {case:?}: expected ingestProtocol {expected_protocol:?}, got {actual_protocol:?}"
        );
        let expected_stage = member(&request, "stageArn");
        let actual_stage = member(&read, "stageArn");
        assert_eq!(
            actual_stage, expected_stage,
            "case {case:?}: expected stageArn {expected_stage:?}, got {actual_stage:?}"
        );
        let expected_name = member(&request, "name");
        let actual_name = member(&read, "name");
        assert_eq!(
            actual_name, expected_name,
            "case {case:?}: expected name {expected_name:?}, got {actual_name:?}"
        );
        let expected_user = member(&request, "userId");
        let actual_user = member(&read, "userId");
        assert_eq!(
            actual_user, expected_user,
            "case {case:?}: expected userId {expected_user:?}, got {actual_user:?}"
        );
        let expected_attributes = request
            .get("attributes")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let actual_attributes = read
            .get("attributes")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        assert_eq!(
            actual_attributes, expected_attributes,
            "case {case:?}: expected attributes {expected_attributes}, got {actual_attributes}"
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let actual_tags = read["tags"].clone();
        assert_eq!(
            actual_tags, expected_tags,
            "case {case:?}: expected tags {expected_tags}, got {actual_tags}"
        );

        // Nobody is publishing to a configuration that has just been created.
        let state = member(&read, "state");
        assert_eq!(
            state, NO_PUBLISHER_STATE,
            "case {case:?}: expected state {NO_PUBLISHER_STATE:?}, got {state:?}"
        );

        // `redundantIngestCredentials` is "present only when redundantIngest is set to
        // true", and each entry carries its own participant id and ingest key.
        let redundant = request
            .get("redundantIngest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let reported_redundant = read
            .get("redundantIngest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(
            reported_redundant, redundant,
            "case {case:?}: expected redundantIngest {redundant}, got {reported_redundant}"
        );
        let credentials = read.get("redundantIngestCredentials").cloned();
        if redundant {
            let entries = credentials
                .as_ref()
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected redundantIngestCredentials, got {read}")
                });
            assert!(
                !entries.is_empty(),
                "case {case:?}: expected at least one redundant credential, got {read}"
            );
            for entry in entries {
                let id = member(entry, "participantId");
                let key = member(entry, "streamKey");
                assert!(
                    matches(PARTICIPANT_ID_PATTERN, &id) && !id.is_empty(),
                    "case {case:?}: expected a redundant participantId matching \
                     {PARTICIPANT_ID_PATTERN}, got {id:?}"
                );
                assert!(
                    matches(STREAM_KEY_PATTERN, &key),
                    "case {case:?}: expected a redundant streamKey matching \
                     {STREAM_KEY_PATTERN}, got {key:?}"
                );
                assert_ne!(
                    key, stream_key,
                    "case {case:?}: a redundant credential must not repeat the primary \
                     ingest key, got {key:?}"
                );
            }
        } else {
            assert!(
                credentials.is_none(),
                "case {case:?}: redundantIngestCredentials is reported only when redundant \
                 ingest is on, got {credentials:?}"
            );
        }
    }
}

/// Every configuration gets its OWN identifiers and its own ingest keys, and the ARN
/// names the region the creating request was signed for.
#[tokio::test]
async fn minted_identifiers_are_distinct_and_carry_the_requested_region() {
    let emulator = RealTime::new();
    // Rows differing in the region the request is signed for, so the region in the ARN
    // is read off the request rather than a constant.
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-1"];
    let mut arns: Vec<String> = Vec::new();
    let mut participants: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();

    for region in regions {
        for (case, request) in create_cases() {
            let case = format!("{case} in {region}");
            let created = emulator.create_in(&case, region, &request).await;
            let configuration_arn = member(&created, "arn");
            let expected_prefix = format!("arn:aws:ivs:{region}:{ACCOUNT}:ingest-configuration/");
            assert!(
                configuration_arn.starts_with(&expected_prefix),
                "case {case:?}: expected an arn under {expected_prefix:?}, got {configuration_arn:?}"
            );
            let stream_key = member(&created, "streamKey");
            assert!(
                stream_key.starts_with(&format!("rt_{ACCOUNT}_{region}_")),
                "case {case:?}: expected a streamKey naming the account and region, \
                 got {stream_key:?}"
            );
            arns.push(configuration_arn);
            participants.push(member(&created, "participantId"));
            keys.push(stream_key);
            for entry in created
                .get("redundantIngestCredentials")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                participants.push(member(entry, "participantId"));
                keys.push(member(entry, "streamKey"));
            }
        }
    }

    for (label, minted) in [
        ("arn", &arns),
        ("participantId", &participants),
        ("streamKey", &keys),
    ] {
        let mut unique = minted.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            minted.len(),
            "case {label:?}: expected {} distinct values, got {}",
            minted.len(),
            unique.len()
        );
    }
}

/// `GetIngestConfiguration` is a report, not a mutation: reading a configuration any
/// number of times answers the same body — the same ingest key included — and leaves the
/// configuration readable.
#[tokio::test]
async fn reading_a_configuration_changes_nothing_a_later_caller_sees() {
    let emulator = RealTime::new();
    for (case, request) in create_cases() {
        let created = emulator.create_in(case, "us-east-1", &request).await;
        let configuration_arn = member(&created, "arn");
        let first = emulator.get(case, &configuration_arn).await;
        for read in 2..=4 {
            let again = emulator.get(case, &configuration_arn).await;
            assert_eq!(
                again, first,
                "case {case:?}: read {read} must answer what read 1 answered, \
                 expected {first}, got {again}"
            );
        }
    }
}

/// An ARN this store holds no configuration for is a 404 `ResourceNotFoundException`
/// stating the identifier the request named — whatever shape of well-formed ARN it is.
#[tokio::test]
async fn an_unheld_arn_is_a_resource_not_found() {
    let emulator = RealTime::new();
    // One live configuration, so the store is not simply empty.
    let live = emulator
        .create_in("a live configuration", "us-east-1", &json!({ "ingestProtocol": "RTMPS" }))
        .await;
    let live_arn = member(&live, "arn");

    let cases = [
        ("never created", arn("us-east-1", "ingest-configuration", "absent")),
        (
            "another region",
            arn("eu-west-2", "ingest-configuration", "absent-too"),
        ),
        (
            "the live id under another account",
            live_arn.replace(ACCOUNT, "123456789012"),
        ),
        (
            "an id one character off a live one",
            format!("{live_arn}0"),
        ),
    ];
    for (case, identifier) in cases {
        let (status, response) = emulator
            .call("GetIngestConfiguration", &json!({ "arn": identifier }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {identifier}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        let expected = format!("Resource: {identifier} not found");
        let message = member(&response, "message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The refused reads left the live configuration exactly where it was.
    let still_there = emulator.get("a live configuration", &live_arn).await;
    assert_eq!(
        still_there, live,
        "expected the live configuration to be unchanged, expected {live}, got {still_there}"
    );
}

/// A value outside the `arn` member's modelled constraints is refused as a
/// `ValidationException` (HTTP 400), not as a not-found and not as a 501.
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
            "an underscore, outside the id character class",
            arn("us-east-1", "ingest-configuration", "abc_def"),
        ),
        (
            "over the @length maximum",
            arn(
                "us-east-1",
                "ingest-configuration",
                &"a".repeat(INGEST_CONFIGURATION_ARN_MAX_LENGTH),
            ),
        ),
    ];
    for (case, identifier) in cases {
        let (status, response) = emulator
            .call("GetIngestConfiguration", &json!({ "arn": identifier }))
            .await;
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
            message.starts_with("GetIngestConfiguration.arn "),
            "case {case:?}: expected a refusal naming the operation and the member, got {message:?}"
        );
    }

    // A request that omits the `@required` member is refused the same way.
    let (status, response) = emulator.call("GetIngestConfiguration", &json!({})).await;
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

/// The documented pairing of `ingestProtocol` and `insecureIngest`: cleartext `RTMP`
/// ingest is refused unless the request allows insecure ingest; the TLS protocol carries
/// no such requirement. Exercised over every combination of the two members, and nothing
/// is persisted for a refused request.
#[tokio::test]
async fn cleartext_ingest_requires_insecure_ingest() {
    let emulator = RealTime::new();
    for protocol in INGEST_PROTOCOLS {
        for insecure in [None, Some(false), Some(true)] {
            let mut request = json!({ "ingestProtocol": protocol });
            if let Some(insecure) = insecure {
                request["insecureIngest"] = Value::Bool(insecure);
            }
            let case = format!("{protocol} with insecureIngest {insecure:?}");
            let refused = protocol == "RTMP" && insecure != Some(true);
            let (status, response) = emulator.call("CreateIngestConfiguration", &request).await;
            if refused {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {response}"
                );
                let message = member(&response, "message");
                assert!(
                    message.starts_with("Request member: ingestProtocol "),
                    "case {case:?}: expected a refusal naming the member the rule is \
                     documented on, got {message:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {response}"
                );
                let created_arn = member(&response["ingestConfiguration"], "arn");
                let read = emulator.get(&case, &created_arn).await;
                let reported = member(&read, "ingestProtocol");
                assert_eq!(
                    reported, protocol,
                    "case {case:?}: expected ingestProtocol {protocol:?}, got {reported:?}"
                );
            }
        }
    }
}

/// The documented 1 KB `attributes` limit: a map at the maximum is persisted and read
/// back, one byte over it is refused, and the refusal persists nothing.
#[tokio::test]
async fn an_oversized_attributes_map_is_refused_at_the_documented_bound() {
    let emulator = RealTime::new();
    let cases = [
        ("the smallest map a request can carry", "{\"a\":\"\"}".len(), true),
        ("half the maximum", MAX_ATTRIBUTES_BYTES / 2, true),
        ("exactly the maximum", MAX_ATTRIBUTES_BYTES, true),
        ("one byte past the maximum", MAX_ATTRIBUTES_BYTES + 1, false),
        ("far past the maximum", MAX_ATTRIBUTES_BYTES * 4, false),
    ];
    for (case, size, accepted) in cases {
        let attributes = attributes_of_size(size);
        let request = json!({ "ingestProtocol": "RTMPS", "attributes": attributes });
        let (status, response) = emulator.call("CreateIngestConfiguration", &request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {size} bytes, got {status} {response}"
            );
            let created_arn = member(&response["ingestConfiguration"], "arn");
            let read = emulator.get(case, &created_arn).await;
            let reported: HashMap<String, String> =
                serde_json::from_value(read["attributes"].clone()).unwrap_or_default();
            assert_eq!(
                reported, attributes,
                "case {case:?}: expected the attributes to be read back, \
                 expected {attributes:?}, got {reported:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {size} bytes, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            let message = member(&response, "message");
            assert!(
                message.starts_with("Request member: attributes "),
                "case {case:?}: expected a refusal naming the member, got {message:?}"
            );
        }
    }
}

/// The tags of an ingest configuration are the same state through either door: what
/// `TagResource` writes on the configuration's ARN is what `GetIngestConfiguration`
/// reports, and what `UntagResource` removes disappears from both.
#[tokio::test]
async fn tags_written_through_the_tagging_surface_are_read_back_on_the_configuration() {
    let emulator = RealTime::new();
    let created = emulator
        .create_in(
            "a tagged configuration",
            "us-east-1",
            &json!({ "ingestProtocol": "RTMPS", "tags": { "env": "test" } }),
        )
        .await;
    let configuration_arn = member(&created, "arn");

    // Rows differing in what the write does: add a key, change a key, add several.
    let cases: [(&str, Value, HashMap<&str, &str>); 3] = [
        (
            "an added key",
            json!({ "tags": { "team": "studio" } }),
            HashMap::from([("env", "test"), ("team", "studio")]),
        ),
        (
            "a changed key",
            json!({ "tags": { "env": "live" } }),
            HashMap::from([("env", "live"), ("team", "studio")]),
        ),
        (
            "several keys at once",
            json!({ "tags": { "a": "1", "b": "2" } }),
            HashMap::from([("env", "live"), ("team", "studio"), ("a", "1"), ("b", "2")]),
        ),
    ];
    for (case, body, expected) in cases {
        let (status, response) = emulator
            .tagging("POST", &configuration_arn, Some(body.clone()))
            .await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource {body} must answer HTTP 204, got {status} {response}"
        );
        let expected: HashMap<String, String> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let read = emulator.get(case, &configuration_arn).await;
        let reported: HashMap<String, String> =
            serde_json::from_value(read["tags"].clone()).unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the configuration to carry {expected:?}, got {reported:?}"
        );
        let (status, listed) = emulator.tagging("GET", &configuration_arn, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer HTTP 200, got {status} {listed}"
        );
        let through_tagging: HashMap<String, String> =
            serde_json::from_value(listed["tags"].clone()).unwrap_or_default();
        assert_eq!(
            through_tagging, expected,
            "case {case:?}: the tagging surface must report what the configuration carries, \
             expected {expected:?}, got {through_tagging:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The stage association is a reference, resolved against the stages this account holds
// ---------------------------------------------------------------------------

/// `stageArn` carries a REFERENCE to a stage of this same service, so a create either
/// names a stage this account holds or names no stage at all: the empty string the
/// modelled `IngestConfigurationStageArn` admits (and an omitted member) is the "no
/// stage" value and is accepted, and every other value has to resolve — a well-formed
/// stage ARN no stage answers for is refused with the modelled `ValidationException`
/// (HTTP 400), creating nothing.
///
/// The rows span what the rule generalises over: whether the member is sent at all, the
/// form and region of the ARN sent, and how it comes to name no stage (never created, or
/// created and then deleted).
#[tokio::test]
async fn a_stage_association_must_name_a_stage_this_account_holds() {
    let emulator = RealTime::new();

    // One stage this account holds, and one it held and deleted, so both an association
    // that resolves and one that stopped resolving are real rather than constructed.
    let (status, held) = emulator.call("CreateStage", &json!({ "name": "live" })).await;
    assert_eq!(
        status, 200,
        "CreateStage must answer HTTP 200, got {status} {held}"
    );
    let held_stage = member(&held["stage"], "arn");

    let (status, doomed) = emulator.call("CreateStage", &json!({ "name": "gone" })).await;
    assert_eq!(
        status, 200,
        "CreateStage must answer HTTP 200, got {status} {doomed}"
    );
    let deleted_stage = member(&doomed["stage"], "arn");
    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": deleted_stage }))
        .await;
    assert_eq!(
        status, 200,
        "DeleteStage must answer HTTP 200, got {status} {response}"
    );

    // One live configuration, so a refusal can be shown to have changed nothing a later
    // caller sees.
    let live = emulator
        .create_in(
            "a live configuration",
            "us-east-1",
            &json!({ "ingestProtocol": "RTMPS" }),
        )
        .await;
    let live_arn = member(&live, "arn");

    // (case, the `stageArn` the request carries, is it accepted, is it a well-formed
    // stage ARN). Every well-formed ARN is also offered to `GetStage`, which answers 200
    // for exactly the rows this operation must accept and 404 for the ones it must
    // refuse — so the two operations agree on which stages exist.
    let cases: [(&str, Option<String>, bool, bool); 8] = [
        ("the member omitted", None, true, false),
        (
            "the empty string, the modelled no-stage value",
            Some(String::new()),
            true,
            false,
        ),
        (
            "a stage this account holds",
            Some(held_stage.clone()),
            true,
            true,
        ),
        (
            "a well-formed stage ARN never created",
            Some(arn("us-east-1", "stage", "neverCreated1")),
            false,
            true,
        ),
        (
            "a one-character stage id never created",
            Some(arn("us-east-1", "stage", "a")),
            false,
            true,
        ),
        (
            "a stage ARN in another region, never created",
            Some(arn("eu-west-1", "stage", "abcdef123456")),
            false,
            true,
        ),
        (
            "a stage created and then deleted",
            Some(deleted_stage.clone()),
            false,
            true,
        ),
        (
            "an ARN of another resource type",
            Some(arn("us-east-1", "ingest-configuration", "abcdef123456")),
            false,
            false,
        ),
    ];

    for (case, requested, accepted, well_formed) in cases {
        let mut request = json!({ "ingestProtocol": "RTMPS" });
        if let Some(stage_arn) = requested.clone() {
            request["stageArn"] = Value::String(stage_arn);
        }
        let (status, response) = emulator.call("CreateIngestConfiguration", &request).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {request}, got {status} {response}"
            );
            // The association is read back off the configuration: what the request named,
            // or the no-stage value for a request that named nothing.
            let expected = requested.clone().unwrap_or_default();
            let created_arn = member(&response["ingestConfiguration"], "arn");
            let read = emulator.get(case, &created_arn).await;
            let reported = member(&read, "stageArn");
            assert_eq!(
                reported, expected,
                "case {case:?}: expected stageArn {expected:?}, got {reported:?}"
            );
            if well_formed {
                // The premise of the acceptance: the ARN this configuration reports
                // really names a stage, which is what `GetStage` answers for it.
                let stage_arn = requested.unwrap_or_default();
                let (status, stage) = emulator
                    .call("GetStage", &json!({ "arn": stage_arn }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetStage on {stage_arn:?} to answer HTTP 200, \
                     got {status} {stage}"
                );
            }
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {request}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let message = member(&response, "message");
        assert!(
            message.starts_with("CreateIngestConfiguration.stageArn "),
            "case {case:?}: expected a refusal naming the operation and the member, \
             got {message:?}"
        );

        if well_formed {
            // The premise of the refusal: this ARN really names no stage, which is what
            // `GetStage` reports for it.
            let stage_arn = requested.clone().unwrap_or_default();
            let (status, stage) = emulator
                .call("GetStage", &json!({ "arn": stage_arn }))
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected GetStage on {stage_arn:?} to answer HTTP 404, \
                 got {status} {stage}"
            );
        }
    }

    // Every refused create left the store as it was: the live configuration still reads
    // back exactly, and the held stage is still held.
    let still_there = emulator.get("a live configuration", &live_arn).await;
    assert_eq!(
        still_there, live,
        "expected the live configuration to be unchanged, expected {live}, got {still_there}"
    );
    let (status, stage) = emulator
        .call("GetStage", &json!({ "arn": held_stage }))
        .await;
    assert_eq!(
        status, 200,
        "expected GetStage on the held stage to answer HTTP 200, got {status} {stage}"
    );
}
