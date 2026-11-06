// SPDX-License-Identifier: CC0-1.0

//! The stage lifecycle over the wire: `CreateStage`, `GetStage`, `ListStages`,
//! `CreateParticipantToken`, `UpdateStage`, `DeleteStage`.
//!
//! Every test drives the service through the router's own `dispatch`, so each
//! assertion is about what an SDK client sees: the HTTP status, the error code and
//! the JSON body — never an internal call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// `StageArn`'s `@pattern`, from
/// `smithy_query(query="shape",shape="Stage")` — member `arn`.
const STAGE_ARN_PATTERN: &str = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$";

/// `ParticipantTokenId`'s `@pattern`, from
/// `smithy_query(query="shape",shape="ParticipantToken")` — member `participantId`.
const PARTICIPANT_ID_PATTERN: &str = "^[a-zA-Z0-9-_]*$";

/// `PaginationToken`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListStages")` — member `nextToken`.
const PAGINATION_TOKEN_PATTERN: &str = "^[a-zA-Z0-9+/=_-]*$";

/// The documented default lifetime of a participant token, in minutes.
const DEFAULT_DURATION: i64 = 720;

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4
    /// credential scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
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

    /// [`Self::call_in`] against the emulator's default region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, body: &Value) -> String {
        let (status, response) = self.call("CreateStage", body).await;
        assert_eq!(status, 200, "CreateStage {body}: got {response}");
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a storage configuration and return the ARN the service minted for it — the
    /// only kind of value `AutoParticipantRecordingConfiguration$storageConfigurationArn`
    /// can name, since it is a reference to a `StorageConfiguration` of this service.
    async fn create_storage_configuration(&self, bucket: &str) -> String {
        let request = json!({ "s3": { "bucketName": bucket }, "name": bucket });
        let (status, response) = self.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "CreateStorageConfiguration {request}: got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The epoch-second instant a modelled `date-time` member denotes.
///
/// The rendering is part of the contract, not an implementation detail: the model gives
/// `ParticipantTokenExpirationTime` `@timestampFormat("date-time")` and its doc says
/// "ISO 8601 timestamp (returned as a string)", so only a STRING is accepted here — a
/// bare epoch number (the restJson1 protocol default, which this service avoids by
/// carrying the format on the member schema `ivsrt_schema_and_codec!` builds, see
/// `src/ops/types/date_time_wire.rs`)
/// fails the parse and the case.
fn timestamp_secs(value: &Value) -> i64 {
    let rendered = value
        .as_str()
        .unwrap_or_else(|| panic!("expected an ISO-8601 date-time string, got {value}"));
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|e| panic!("{rendered:?} is not an ISO-8601 date-time: {e}"))
        .secs()
}

/// Does `value` satisfy one of the three model `@pattern`s this suite checks?
///
/// The patterns are quoted on the constants above, straight from `smithy_query`, and are
/// simple enough to check directly — which keeps this crate's dependency set exactly what
/// the service itself needs. Each arm names the pattern it implements; a pattern with no
/// arm panics rather than passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        // ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$
        STAGE_ARN_PATTERN => {
            let Some(rest) = value.strip_prefix("arn:aws:ivs:") else {
                return false;
            };
            let Some((region, rest)) = rest.split_once(':') else {
                return false;
            };
            let Some((account, resource)) = rest.split_once(':') else {
                return false;
            };
            let Some(id) = resource.strip_prefix("stage/") else {
                return false;
            };
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !account.is_empty()
                && account.chars().all(|c| c.is_ascii_digit())
                && !id.is_empty()
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
        // ^[a-zA-Z0-9-_]*$
        PARTICIPANT_ID_PATTERN => value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        // ^[a-zA-Z0-9+/=_-]*$
        PAGINATION_TOKEN_PATTERN => value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)),
        other => panic!("this suite has no checker for the pattern {other:?}"),
    }
}

/// A `participantTokenConfigurations` entry.
fn token_configuration(user_id: &str) -> Value {
    json!({ "userId": user_id })
}

// ---------------------------------------------------------------------------
// CreateStage
// ---------------------------------------------------------------------------

/// CreateStage mints a stage ARN of the modelled form and issues exactly one
/// participant token per `participantTokenConfigurations` entry — over the whole
/// range the model allows (`len 0..12`), not just the one-entry case.
#[tokio::test]
async fn create_stage_mints_an_arn_and_one_token_per_configuration() {
    let emulator = Emulator::new();
    let twelve: Vec<Value> = (0..12)
        .map(|index| token_configuration(&format!("user-{index}")))
        .collect();
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no configurations member", json!({ "name": "demo-stage" }), 0),
        (
            "an empty configuration list",
            json!({ "name": "demo-stage", "participantTokenConfigurations": [] }),
            0,
        ),
        (
            "one configuration",
            json!({
                "name": "demo-stage",
                "participantTokenConfigurations": [
                    { "userId": "alice", "duration": 60, "capabilities": ["PUBLISH"] }
                ],
            }),
            1,
        ),
        (
            "three configurations",
            json!({
                "participantTokenConfigurations": [
                    token_configuration("a"),
                    token_configuration("b"),
                    token_configuration("c"),
                ],
            }),
            3,
        ),
        (
            "the modelled maximum of twelve",
            json!({ "participantTokenConfigurations": twelve }),
            12,
        ),
    ];

    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_participant_ids: BTreeSet<String> = BTreeSet::new();
    // Every token string this emulator has issued so far, across every case: a token is
    // minted per issuance, so two participants never share one.
    let mut seen_tokens: BTreeSet<String> = BTreeSet::new();
    for (case, request, expected_tokens) in cases {
        let (status, response) = emulator.call("CreateStage", &request).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {response}");

        let arn = response["stage"]["arn"].as_str().unwrap_or_default();
        assert!(
            matches(STAGE_ARN_PATTERN, arn),
            "case {case:?}: expected an arn matching {STAGE_ARN_PATTERN}, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn.to_string()),
            "case {case:?}: expected a fresh stage arn, got the already-minted {arn:?}"
        );

        let tokens = response["participantTokens"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tokens.len(),
            expected_tokens,
            "case {case:?}: expected {expected_tokens} participant tokens, got {response}"
        );
        // The model's doc for `participantTokens` — "Participant tokens attached to the
        // stage. These correspond to the participants in the request" — makes the member
        // a report of the request's participants, so a request that named none has
        // nothing to report and the member is absent rather than an empty list.
        assert_eq!(
            response["participantTokens"].is_null(),
            expected_tokens == 0,
            "case {case:?}: expected participantTokens to be {}, got {response}",
            if expected_tokens == 0 {
                "absent"
            } else {
                "present"
            }
        );
        for token in &tokens {
            let participant_id = token["participantId"].as_str().unwrap_or_default();
            assert!(
                !participant_id.is_empty()
                    && matches(PARTICIPANT_ID_PATTERN, participant_id)
                    && participant_id.len() <= 64,
                "case {case:?}: expected a participantId matching \
                 {PARTICIPANT_ID_PATTERN}, got {participant_id:?}"
            );
            assert!(
                seen_participant_ids.insert(participant_id.to_string()),
                "case {case:?}: expected a unique participantId, got the \
                 already-issued {participant_id:?}"
            );
            let issued = token["token"].as_str().unwrap_or_default();
            assert!(
                !issued.is_empty(),
                "case {case:?}: expected a non-empty token, got {token}"
            );
            assert!(
                seen_tokens.insert(issued.to_string()),
                "case {case:?}: expected a token minted for this participant alone, \
                 got the already-issued {issued:?}"
            );
        }
    }
}

/// One row of a reported-token table: the case, the row's own `Value` (the request it
/// sends, or the `userId` its token must report), and the `duration` and `capabilities`
/// the token must report — `None` where the request carried the member not at all.
type ReportedTokenRow = (
    &'static str,
    Value,
    Option<i64>,
    Option<Vec<&'static str>>,
);

/// Each configuration's own values reach its token, the model's documented
/// defaults fill in what the configuration leaves out, and a default the service applied
/// is not echoed back as the member the configuration left out.
#[tokio::test]
async fn create_stage_applies_each_configuration_to_its_own_token() {
    let emulator = Emulator::new();
    let request = json!({
        "name": "mixed-stage",
        "participantTokenConfigurations": [
            { "userId": "alice", "duration": 60, "capabilities": ["PUBLISH"] },
            { "userId": "bob" },
            { "duration": 20160, "capabilities": ["PUBLISH", "SUBSCRIBE"],
              "attributes": { "seat": "1a" } },
        ],
    });
    let issued_at = now_secs();
    let (status, response) = emulator.call("CreateStage", &request).await;
    assert_eq!(status, 200, "CreateStage: expected 200, got {response}");
    let tokens = response["participantTokens"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(tokens.len(), 3, "expected three tokens, got {response}");

    // The third and fourth columns are what the token must REPORT for `duration` and
    // `capabilities`: `None` where the configuration carried the member not at all.
    let expected: Vec<ReportedTokenRow> = vec![
        (
            "explicit duration and one capability",
            json!("alice"),
            Some(60),
            Some(vec!["PUBLISH"]),
        ),
        (
            "defaults for duration and capabilities",
            json!("bob"),
            None,
            None,
        ),
        (
            "the modelled maximum duration, no userId",
            Value::Null,
            Some(20160),
            Some(vec!["PUBLISH", "SUBSCRIBE"]),
        ),
    ];
    for (index, (case, user_id, duration, capabilities)) in expected.into_iter().enumerate() {
        let token = &tokens[index];
        assert_eq!(
            token["userId"], user_id,
            "case {case:?}: expected userId {user_id}, got {token}"
        );
        assert_eq!(
            token.get("duration").and_then(Value::as_i64),
            duration,
            "case {case:?}: expected duration {duration:?}, got {token}"
        );
        let reported: Option<Vec<&str>> = token.get("capabilities").map(|values| {
            values
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default()
        });
        assert_eq!(
            reported, capabilities,
            "case {case:?}: expected capabilities {capabilities:?}, got {reported:?}"
        );
        // Whatever the token reports, its expiry is computed from the EFFECTIVE duration:
        // the documented default where the configuration sized nothing.
        let expiry = timestamp_secs(&token["expirationTime"]);
        let minutes = duration.unwrap_or(DEFAULT_DURATION);
        assert!(
            (expiry - (issued_at + minutes * 60)).abs() <= 120,
            "case {case:?}: expected an expirationTime {minutes} minutes out, got {token}"
        );
    }
    assert_eq!(
        tokens[2]["attributes"],
        json!({ "seat": "1a" }),
        "a configuration's attributes must reach its token, got {}",
        tokens[2]
    );
}

/// A minted ARN names the region the request was signed for.
#[tokio::test]
async fn create_stage_names_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let (status, response) = emulator
            .call_in(region, "CreateStage", &json!({ "name": "regional" }))
            .await;
        assert_eq!(status, 200, "region {region:?}: expected 200, got {response}");
        let arn = response["stage"]["arn"].as_str().unwrap_or_default();
        assert!(
            arn.starts_with(&format!("arn:aws:ivs:{region}:")),
            "region {region:?}: expected an arn in that region, got {arn:?}"
        );
        assert!(
            matches(STAGE_ARN_PATTERN, arn),
            "region {region:?}: expected an arn matching {STAGE_ARN_PATTERN}, got {arn:?}"
        );
    }
}

/// The documented 1 KB cap on a token's `attributes` is enforced, and a map under
/// it is accepted — on both operations that take one.
#[tokio::test]
async fn attributes_over_one_kilobyte_are_refused() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage(&json!({ "name": "attrs" })).await;

    // Rows differ only in the size of the map as JSON, which is what the rule bounds.
    let cases: Vec<(&str, usize, u16)> = vec![
        ("well under the cap", 100, 200),
        ("exactly at the cap", 1024, 200),
        ("one byte over the cap", 1025, 400),
        ("far over the cap", 4096, 400),
    ];
    for (case, total_bytes, expected_status) in cases {
        let key = "a";
        // The measured size is the map's JSON encoding, so the value carries the target
        // minus the punctuation around one entry (`{"a":"…"}`).
        let value = "v".repeat(total_bytes - "{\"a\":\"\"}".len());
        let attributes = json!({ key: value });

        let (status, response) = emulator
            .call(
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn, "attributes": attributes }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "CreateParticipantToken, case {case:?} ({total_bytes} bytes): expected \
             {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "CreateParticipantToken, case {case:?}: got {response}"
            );
        }

        let (status, response) = emulator
            .call(
                "CreateStage",
                &json!({
                    "participantTokenConfigurations": [{ "attributes": attributes }],
                }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "CreateStage, case {case:?} ({total_bytes} bytes): expected \
             {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "CreateStage, case {case:?}: got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetStage
// ---------------------------------------------------------------------------

/// GetStage reports the stage CreateStage persisted, member for member, whatever
/// the creating request supplied.
#[tokio::test]
async fn get_stage_reads_back_what_create_stage_returned() {
    let emulator = Emulator::new();
    // A recording configuration names a storage configuration of this service, so the
    // row that attaches one creates it first.
    let storage = emulator.create_storage_configuration("recordings").await;
    let cases: Vec<(&str, Value)> = vec![
        ("name only", json!({ "name": "demo-stage" })),
        ("no members at all", json!({})),
        (
            "name and tags",
            json!({ "name": "tagged", "tags": { "team": "video", "env": "test" } }),
        ),
        (
            "a recording configuration",
            json!({
                "name": "recorded",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": storage.as_str(),
                    "mediaTypes": ["AUDIO_VIDEO"],
                    "recordingReconnectWindowSeconds": 30,
                },
            }),
        ),
        (
            "tokens as well as a name",
            json!({
                "name": "with-tokens",
                "participantTokenConfigurations": [token_configuration("alice")],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, created) = emulator.call("CreateStage", &request).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {created}");
        let arn = created["stage"]["arn"].as_str().unwrap_or_default();

        let (status, read) = emulator.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {read}");
        assert_eq!(
            read["stage"], created["stage"],
            "case {case:?}: expected GetStage to report the created stage, got {read}"
        );
    }
}

/// A well-formed ARN naming no stage is a 404 `ResourceNotFoundException`; an ARN
/// the model's pattern refuses is a 400 `ValidationException`.
#[tokio::test]
async fn get_stage_separates_an_unknown_stage_from_a_malformed_arn() {
    let emulator = Emulator::new();
    let live = emulator.create_stage(&json!({ "name": "live" })).await;

    let cases: Vec<(&str, &str, u16, &str)> = vec![
        ("the stage that exists", &live, 200, ""),
        (
            "a well-formed arn naming no stage",
            "arn:aws:ivs:us-east-1:000000000000:stage/deadbeef0000",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed arn in another region",
            "arn:aws:ivs:eu-west-1:000000000000:stage/deadbeef0000",
            404,
            "ResourceNotFoundException",
        ),
        ("not an arn at all", "not-an-arn", 400, "ValidationException"),
        (
            "an arn of another service",
            "arn:aws:s3:us-east-1:000000000000:stage/abcd1234",
            400,
            "ValidationException",
        ),
        (
            "an arn with no resource id",
            "arn:aws:ivs:us-east-1:000000000000:stage/",
            400,
            "ValidationException",
        ),
        ("an empty arn", "", 400, "ValidationException"),
    ];

    for (case, arn, expected_status, expected_code) in cases {
        let (status, response) = emulator.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListStages
// ---------------------------------------------------------------------------

/// Whatever the page bound, paging through `ListStages` yields every stored stage
/// exactly once, in creation order, and only the last page omits `nextToken`.
#[tokio::test]
async fn list_stages_pages_over_every_stage_in_creation_order() {
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..7 {
        created.push(
            emulator
                .create_stage(&json!({ "name": format!("stage-{index}") }))
                .await,
        );
    }

    // Rows differ in the page bound, which is what the rule generalises over:
    // every bound below, at and above the stored count, plus the absent member.
    for max_results in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(100), None] {
        let case = format!("maxResults={max_results:?}");
        let bound = max_results.unwrap_or(50) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, response) = emulator.call("ListStages", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {response}"
            );
            let page = response["stages"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= bound,
                "case {case}: expected at most {bound} stages per page, got {}",
                page.len()
            );
            for summary in &page {
                let arn = summary["arn"].as_str().unwrap_or_default();
                assert!(
                    matches(STAGE_ARN_PATTERN, arn),
                    "case {case}: expected a summary arn matching \
                     {STAGE_ARN_PATTERN}, got {arn:?}"
                );
                seen.push(arn.to_string());
            }

            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match response["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        matches(PAGINATION_TOKEN_PATTERN, token),
                        "case {case}: expected a nextToken matching \
                         {PAGINATION_TOKEN_PATTERN}, got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        bound,
                        "case {case}: expected a full page before a nextToken, got {response}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, created,
            "case {case}: expected every created stage once, in creation order"
        );
    }
}

/// A `nextToken` resumes where the page ended even when the stage that ended it is
/// deleted in between, and a deleted stage never appears in a page.
#[tokio::test]
async fn list_stages_resumes_after_a_deleted_stage() {
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..4 {
        created.push(
            emulator
                .create_stage(&json!({ "name": format!("s{index}") }))
                .await,
        );
    }

    let (status, first) = emulator
        .call("ListStages", &json!({ "maxResults": 2 }))
        .await;
    assert_eq!(status, 200, "first page: got {first}");
    let token = first["nextToken"]
        .as_str()
        .expect("a nextToken while stages remain")
        .to_string();

    // Delete the stage the first page ended on, then resume.
    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": created[1] }))
        .await;
    assert_eq!(status, 200, "DeleteStage: got {status} {response}");

    let (status, second) = emulator
        .call("ListStages", &json!({ "maxResults": 2, "nextToken": token }))
        .await;
    assert_eq!(status, 200, "second page: got {second}");
    let arns: Vec<&str> = second["stages"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|summary| summary["arn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![created[2].as_str(), created[3].as_str()],
        "expected the page after the deleted one to resume where it ended, got {second}"
    );
}

/// A pagination token `ListStages` did not issue names no position, so it is
/// refused rather than read as "start again".
#[tokio::test]
async fn list_stages_refuses_a_pagination_token_it_did_not_issue() {
    let emulator = Emulator::new();
    emulator.create_stage(&json!({ "name": "only" })).await;

    // Rows differ in how the token is wrong: not base64, base64 of foreign text,
    // base64 of this service's prefix with a non-numeric position.
    let cases: Vec<(&str, &str, u16)> = vec![
        ("not base64 at all", "++++", 400),
        ("base64 of foreign text", "c29tZXRoaW5nLWVsc2U=", 400),
        (
            "base64 of a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0YWdlLWFmdGVyOm5vcGU=",
            400,
        ),
        ("the empty token, which means the first page", "", 200),
    ];
    for (case, token, expected_status) in cases {
        let (status, response) = emulator
            .call("ListStages", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateParticipantToken
// ---------------------------------------------------------------------------

/// The token reflects the request, the model's documented defaults fill in what the
/// request omits, and `expirationTime` is the issue instant plus the effective duration.
///
/// A default the SERVICE applied is not echoed back as the member the caller left out:
/// `duration` and `capabilities` are reported exactly when the request carried them, so a
/// client can tell "the caller sized this token" from "the service defaulted it". What
/// the default did is visible in `expirationTime`, which lands the documented 720 minutes
/// out for a request that sized nothing.
#[tokio::test]
async fn create_participant_token_reflects_the_request_and_its_defaults() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage(&json!({ "name": "tokens" })).await;

    // Rows differ along every member the rule covers: duration (absent, both
    // modelled bounds, a middle value), capabilities (absent, one, both), userId
    // and attributes (absent, present). The third and fourth columns are what the
    // RESPONSE must carry for `duration` and `capabilities` — `None` where the request
    // carried the member not at all.
    let cases: Vec<ReportedTokenRow> = vec![
        (
            "every member omitted",
            json!({ "stageArn": stage_arn }),
            None,
            None,
        ),
        (
            "userId only, so both defaults apply",
            json!({ "stageArn": stage_arn, "userId": "bob" }),
            None,
            None,
        ),
        (
            "the modelled minimum duration",
            json!({ "stageArn": stage_arn, "duration": 1, "capabilities": ["SUBSCRIBE"] }),
            Some(1),
            Some(vec!["SUBSCRIBE"]),
        ),
        (
            "a middling duration and both capabilities",
            json!({
                "stageArn": stage_arn,
                "duration": 90,
                "userId": "carol",
                "capabilities": ["PUBLISH", "SUBSCRIBE"],
                "attributes": { "seat": "12b" },
            }),
            Some(90),
            Some(vec!["PUBLISH", "SUBSCRIBE"]),
        ),
        (
            "the modelled maximum duration",
            json!({ "stageArn": stage_arn, "duration": 20160 }),
            Some(20160),
            None,
        ),
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    // Every token string issued in this test: each is derived from its own record, so
    // no two issuances against the same stage share one.
    let mut seen_tokens: BTreeSet<String> = BTreeSet::new();
    for (case, request, duration, capabilities) in cases {
        let issued_at = now_secs();
        let (status, response) = emulator.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        let token = &response["participantToken"];

        assert_eq!(
            token.get("duration").and_then(Value::as_i64),
            duration,
            "case {case:?}: expected duration {duration:?}, got {token}"
        );
        let reported_capabilities: Option<Vec<&str>> = token.get("capabilities").map(|values| {
            values
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default()
        });
        assert_eq!(
            reported_capabilities, capabilities,
            "case {case:?}: expected capabilities {capabilities:?}, \
             got {reported_capabilities:?}"
        );
        assert_eq!(
            token["userId"], request["userId"],
            "case {case:?}: expected the requested userId, got {token}"
        );
        if let Some(attributes) = request.get("attributes") {
            assert_eq!(
                &token["attributes"], attributes,
                "case {case:?}: expected the requested attributes, got {token}"
            );
        }

        let participant_id = token["participantId"].as_str().unwrap_or_default();
        assert!(
            !participant_id.is_empty() && matches(PARTICIPANT_ID_PATTERN, participant_id),
            "case {case:?}: expected a participantId matching \
             {PARTICIPANT_ID_PATTERN}, got {participant_id:?}"
        );
        assert!(
            seen.insert(participant_id.to_string()),
            "case {case:?}: expected a unique participantId, got the already-issued \
             {participant_id:?}"
        );
        let issued = token["token"].as_str().unwrap_or_default();
        assert!(
            !issued.is_empty(),
            "case {case:?}: expected a non-empty token, got {token}"
        );
        assert!(
            seen_tokens.insert(issued.to_string()),
            "case {case:?}: expected a token minted for this participant alone, \
             got the already-issued {issued:?}"
        );

        // `expirationTime` is the issue instant plus the EFFECTIVE duration — the one the
        // request asked for, or the documented default where it asked for none — rendered
        // as the ISO-8601 date-time string the model's `@timestampFormat` requires.
        let rendered = &token["expirationTime"];
        assert!(
            rendered.is_string(),
            "case {case:?}: expected expirationTime as an ISO-8601 string, got {rendered}"
        );
        let actual = timestamp_secs(rendered);
        let effective = duration.unwrap_or(DEFAULT_DURATION);
        let expected = issued_at + effective * 60;
        assert!(
            (actual - expected).abs() <= 120,
            "case {case:?}: expected an expirationTime near {expected} \
             ({effective} minutes out), got {actual} ({rendered})"
        );
    }
}

/// A duration outside the modelled range is refused, and both published bounds are
/// accepted.
#[tokio::test]
async fn create_participant_token_holds_the_modeled_duration_range() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage(&json!({ "name": "durations" })).await;

    // The model gives `ParticipantTokenDurationMinutes` range 1..20160: each bound,
    // and one step past each bound.
    let cases: Vec<(&str, i64, u16)> = vec![
        ("one below the minimum", 0, 400),
        ("the minimum", 1, 200),
        ("the maximum", 20160, 200),
        ("one above the maximum", 20161, 400),
        ("far below the minimum", -5, 400),
    ];
    for (case, duration, expected_status) in cases {
        let (status, response) = emulator
            .call(
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn, "duration": duration }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (duration {duration}): expected {expected_status}, got \
             {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: got {response}"
            );
        }
    }
}

/// A token can only be issued against a stage that exists.
#[tokio::test]
async fn create_participant_token_refuses_a_stage_that_does_not_exist() {
    let emulator = Emulator::new();
    let live = emulator.create_stage(&json!({ "name": "live" })).await;
    let gone = emulator.create_stage(&json!({ "name": "gone" })).await;
    let (status, response) = emulator.call("DeleteStage", &json!({ "arn": gone })).await;
    assert_eq!(status, 200, "DeleteStage: got {status} {response}");

    let cases: Vec<(&str, &str, u16, &str)> = vec![
        ("a live stage", &live, 200, ""),
        ("a deleted stage", &gone, 404, "ResourceNotFoundException"),
        (
            "a stage that never existed",
            "arn:aws:ivs:us-east-1:000000000000:stage/0123456789ab",
            404,
            "ResourceNotFoundException",
        ),
        ("a malformed arn", "stage/nope", 400, "ValidationException"),
    ];
    for (case, stage_arn, expected_status, expected_code) in cases {
        let (status, response) = emulator
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// UpdateStage
// ---------------------------------------------------------------------------

/// UpdateStage changes the members the request supplies, leaves every other member
/// as stored, and a following read reports the update.
#[tokio::test]
async fn update_stage_changes_only_the_members_supplied() {
    let emulator = Emulator::new();
    // The configuration an update attaches names a storage configuration of this
    // service, so it is created first and the update names the ARN that create minted.
    let storage = emulator.create_storage_configuration("updates").await;
    let recording = json!({
        "storageConfigurationArn": storage.as_str(),
        "mediaTypes": ["AUDIO_ONLY"],
        "recordingReconnectWindowSeconds": 15,
    });
    // What a read reports back: the members the request supplied, plus every member it
    // left out at the default the model documents for it — `recordParticipantReplicas`
    // included, whose doc states "Default: `true`".
    let stored_recording = json!({
        "storageConfigurationArn": storage.as_str(),
        "mediaTypes": ["AUDIO_ONLY"],
        "recordingReconnectWindowSeconds": 15,
        "recordParticipantReplicas": true,
        "thumbnailConfiguration": {
            "targetIntervalSeconds": 60,
            "storage": ["SEQUENTIAL"],
            "recordingMode": "DISABLED",
        },
        "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
    });

    // Rows differ in WHICH members the update supplies.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "name only",
            json!({ "name": "renamed" }),
            json!("renamed"),
            default_recording_configuration(),
        ),
        (
            "recording configuration only",
            json!({ "autoParticipantRecordingConfiguration": recording }),
            json!("original"),
            stored_recording.clone(),
        ),
        (
            "both members",
            json!({ "name": "both", "autoParticipantRecordingConfiguration": recording }),
            json!("both"),
            stored_recording.clone(),
        ),
        (
            "no member at all",
            json!({}),
            json!("original"),
            default_recording_configuration(),
        ),
    ];

    for (case, update, expected_name, expected_recording) in cases {
        let arn = emulator
            .create_stage(&json!({ "name": "original", "tags": { "team": "video" } }))
            .await;
        let mut request = update;
        request["arn"] = json!(arn);

        let (status, updated) = emulator.call("UpdateStage", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["stage"]["name"], expected_name,
            "case {case:?}: expected name {expected_name}, got {updated}"
        );
        assert_eq!(
            updated["stage"]["autoParticipantRecordingConfiguration"], expected_recording,
            "case {case:?}: expected recording configuration {expected_recording}, got \
             {updated}"
        );
        assert_eq!(
            updated["stage"]["tags"],
            json!({ "team": "video" }),
            "case {case:?}: expected the stored tags to survive the update, got {updated}"
        );

        let (status, read) = emulator.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: GetStage got {status} {read}");
        assert_eq!(
            read["stage"], updated["stage"],
            "case {case:?}: expected GetStage to report the update, got {read}"
        );

        let (status, listed) = emulator.call("ListStages", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListStages got {status} {listed}");
        let names: Vec<&str> = listed["stages"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter(|summary| summary["arn"] == json!(arn))
                    .filter_map(|summary| summary["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec![expected_name.as_str().unwrap_or_default()],
            "case {case:?}: expected ListStages to report the update, got {listed}"
        );
    }
}

/// UpdateStage refuses a stage that does not exist, and a malformed ARN.
#[tokio::test]
async fn update_stage_refuses_a_stage_that_does_not_exist() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a well-formed arn naming no stage",
            "arn:aws:ivs:us-east-1:000000000000:stage/abcdef012345",
            404,
            "ResourceNotFoundException",
        ),
        ("a malformed arn", "stage-1", 400, "ValidationException"),
    ];
    for (case, arn, expected_status, expected_code) in cases {
        let (status, response) = emulator
            .call("UpdateStage", &json!({ "arn": arn, "name": "renamed" }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteStage
// ---------------------------------------------------------------------------

/// Deleting a stage removes it and the tokens issued against it; every later
/// reference to it is a 404, and the stages around it are untouched.
#[tokio::test]
async fn delete_stage_removes_the_stage_and_its_tokens() {
    let emulator = Emulator::new();
    let kept = emulator.create_stage(&json!({ "name": "kept" })).await;
    let doomed = emulator
        .create_stage(&json!({
            "name": "doomed",
            "participantTokenConfigurations": [token_configuration("alice")],
        }))
        .await;

    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": doomed }))
        .await;
    assert_eq!(status, 200, "DeleteStage: expected 200, got {status} {response}");
    assert!(
        response.is_null() || response == json!({}),
        "DeleteStage: expected an empty body, got {response}"
    );

    // Every later reference to the deleted stage.
    for (operation, request) in [
        ("GetStage", json!({ "arn": doomed })),
        ("UpdateStage", json!({ "arn": doomed, "name": "zombie" })),
        ("DeleteStage", json!({ "arn": doomed })),
        ("CreateParticipantToken", json!({ "stageArn": doomed })),
    ] {
        let (status, response) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 404,
            "{operation} after delete: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "{operation} after delete: got {response}"
        );
    }

    let (status, listed) = emulator.call("ListStages", &json!({})).await;
    assert_eq!(status, 200, "ListStages: got {status} {listed}");
    let arns: Vec<&str> = listed["stages"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|summary| summary["arn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![kept.as_str()],
        "expected only the surviving stage to be listed, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------

/// Both stores this slice grows are bounded, and a request past a bound is answered
/// with the modelled `ServiceQuotaExceededException` (HTTP 402) rather than growing
/// the store — and deleting a record frees the slot again.
#[tokio::test]
async fn the_stores_are_bounded_and_report_the_modeled_quota_error() {
    // Read from the crate rather than duplicated here, so the test cannot drift from
    // the bounds the service actually enforces. Filling both stores costs about half
    // a second of in-process dispatches, which buys the only assertion that proves
    // the bound is real.
    use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state::{
        MAX_PARTICIPANT_TOKENS_PER_STAGE, MAX_STAGES,
    };

    let emulator = Emulator::new();

    // Growth site 1: CreateStage. Fill the store to its bound, then one more.
    let mut arns = Vec::with_capacity(MAX_STAGES);
    for index in 0..MAX_STAGES {
        let (status, response) = emulator.call("CreateStage", &json!({})).await;
        assert_eq!(
            status, 200,
            "stage {index} of {MAX_STAGES}: expected 200, got {status} {response}"
        );
        arns.push(
            response["stage"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let (status, response) = emulator.call("CreateStage", &json!({})).await;
    assert_eq!(
        status, 402,
        "one stage past the bound: expected 402, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "one stage past the bound: got {response}"
    );

    // Deleting one frees the slot: the bound is a cap on what is stored, not a
    // lifetime counter.
    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": arns[0] }))
        .await;
    assert_eq!(status, 200, "DeleteStage: got {status} {response}");
    let (status, response) = emulator.call("CreateStage", &json!({})).await;
    assert_eq!(
        status, 200,
        "after freeing a slot: expected 200, got {status} {response}"
    );
    let stage_arn = response["stage"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Growth site 2: CreateParticipantToken against one stage.
    let request = json!({ "stageArn": stage_arn });
    for index in 0..MAX_PARTICIPANT_TOKENS_PER_STAGE {
        let (status, response) = emulator.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "token {index} of {MAX_PARTICIPANT_TOKENS_PER_STAGE}: expected 200, got \
             {status} {response}"
        );
    }
    let (status, response) = emulator.call("CreateParticipantToken", &request).await;
    assert_eq!(
        status, 402,
        "one token past the per-stage bound: expected 402, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "one token past the per-stage bound: got {response}"
    );

    // And deleting the stage releases every token record it held, so a fresh stage
    // starts from an empty slot again.
    let (status, response) = emulator
        .call("DeleteStage", &json!({ "arn": stage_arn }))
        .await;
    assert_eq!(status, 200, "DeleteStage: got {status} {response}");
    let fresh = emulator.create_stage(&json!({})).await;
    let (status, response) = emulator
        .call("CreateParticipantToken", &json!({ "stageArn": fresh }))
        .await;
    assert_eq!(
        status, 200,
        "a fresh stage after the bound was reached: expected 200, got {status} {response}"
    );
}

// ---------------------------------------------------------------------------
// The Stage and ParticipantToken projections
// ---------------------------------------------------------------------------

/// Every member of `AutoParticipantRecordingConfiguration` at the default the model
/// documents for it: what a stage reports when its request configured no storage.
///
/// Sources — `smithy_query(query="shape",
/// shape="AutoParticipantRecordingConfiguration,ParticipantThumbnailConfiguration,
/// ParticipantRecordingHlsConfiguration")`: `storageConfigurationArn` "Default: `""`
/// (empty string, no storage configuration is specified)", `mediaTypes` "Default:
/// AUDIO_VIDEO", `recordingReconnectWindowSeconds` "The default value is 0",
/// `recordParticipantReplicas` "Default: true", `thumbnailConfiguration`
/// `targetIntervalSeconds` "Default: 60" / `storage` "Default: SEQUENTIAL" /
/// `recordingMode` "Default: DISABLED", and `hlsConfiguration`
/// `targetSegmentDurationSeconds` "Default: 6".
fn default_recording_configuration() -> Value {
    json!({
        "storageConfigurationArn": "",
        "mediaTypes": ["AUDIO_VIDEO"],
        "recordingReconnectWindowSeconds": 0,
        "recordParticipantReplicas": true,
        "thumbnailConfiguration": {
            "targetIntervalSeconds": 60,
            "storage": ["SEQUENTIAL"],
            "recordingMode": "DISABLED",
        },
        "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
    })
}

/// All four `StageEndpoints` members are present on `stage`, and each is well-formed for
/// its protocol: the scheme, the per-stage host and the path are what a client parses out
/// of an endpoint. The HOSTS themselves are environment-specific and are not asserted.
fn assert_stage_endpoints(case: &str, stage: &Value) {
    let arn = stage["arn"].as_str().unwrap_or_default();
    let stage_id = arn.rsplit('/').next().unwrap_or_default();
    assert!(
        !stage_id.is_empty(),
        "case {case:?}: expected the stage arn to carry a resource id, got {arn:?}"
    );
    let endpoints = &stage["endpoints"];

    // The events endpoint is one global host carrying no per-stage part.
    let events = endpoints["events"].as_str();
    assert!(
        events.is_some_and(|value| value.starts_with("wss://")),
        "case {case:?}: expected a wss:// events endpoint, got {events:?}"
    );

    // Rows: the member, its scheme, and the path a client publishes to.
    for (member, scheme, path) in [
        ("whip", "https://", ""),
        ("rtmp", "rtmp://", "/app/"),
        ("rtmps", "rtmps://", "/app/"),
    ] {
        let actual = endpoints[member].as_str();
        assert!(
            actual.is_some_and(|value| value.starts_with(scheme)
                && value.ends_with(path)
                && value.contains(stage_id)),
            "case {case:?}: expected {member} to start with {scheme:?}, end with {path:?} \
             and name the stage id {stage_id:?}, got {actual:?}"
        );
    }
}

/// Every stage reports its four data-plane endpoints, a recording configuration with the
/// modelled defaults for whatever the request left out, and an `activeSessionId` — and
/// `CreateStage` and `GetStage` report one identical stage.
#[tokio::test]
async fn every_stage_projection_carries_endpoints_a_recording_configuration_and_a_session_id() {
    let emulator = Emulator::new();
    // The row that configures storage names a storage configuration of this service, so
    // it is created first and the request names the ARN that create minted.
    let storage = emulator.create_storage_configuration("projections").await;

    // Rows differ in which members the creating request supplies, and in whether it
    // configures storage for individual-participant recording.
    let configured = json!({
        "storageConfigurationArn": storage.as_str(),
        "mediaTypes": ["AUDIO_ONLY"],
        "recordingReconnectWindowSeconds": 30,
    });
    let cases: Vec<(&str, Value, Value)> = vec![
        ("no members at all", json!({}), default_recording_configuration()),
        (
            "a name",
            json!({ "name": "endpoints-demo" }),
            default_recording_configuration(),
        ),
        (
            "a name and tags",
            json!({ "name": "tagged", "tags": { "team": "video" } }),
            default_recording_configuration(),
        ),
        (
            "participant tokens as well",
            json!({ "participantTokenConfigurations": [{ "userId": "alice" }] }),
            default_recording_configuration(),
        ),
        (
            // Documented on `storageConfigurationArn`: "other fields in this object will
            // get reset to their defaults when sending `\"\"`".
            "a recording configuration with no storage configured",
            json!({
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": "",
                    "mediaTypes": ["NONE"],
                    "recordingReconnectWindowSeconds": 120,
                },
            }),
            default_recording_configuration(),
        ),
        (
            "a recording configuration with storage configured",
            json!({ "autoParticipantRecordingConfiguration": configured }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_ONLY"],
                "recordingReconnectWindowSeconds": 30,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 60,
                    "storage": ["SEQUENTIAL"],
                    "recordingMode": "DISABLED",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
            }),
        ),
    ];

    for (case, request, expected_recording) in cases {
        let (status, created) = emulator.call("CreateStage", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created}"
        );
        let created_stage = created["stage"].clone();
        assert_stage_endpoints(case, &created_stage);
        assert_eq!(
            created_stage["activeSessionId"],
            json!(""),
            "case {case:?}: expected an empty activeSessionId on a stage with no \
             session, got {created_stage}"
        );
        assert_eq!(
            created_stage["autoParticipantRecordingConfiguration"], expected_recording,
            "case {case:?}: expected recording configuration {expected_recording}, got \
             {created_stage}"
        );

        // The create response is what the model tells a caller to cache, so a read must
        // report exactly it.
        let arn = created_stage["arn"].as_str().unwrap_or_default().to_string();
        let (status, read) = emulator.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: GetStage got {status} {read}");
        assert_eq!(
            read["stage"], created_stage,
            "case {case:?}: expected GetStage to report the created stage, got {read}"
        );

        // The summary of the same stage answers alike on the member it shares.
        let (status, listed) = emulator.call("ListStages", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListStages got {status} {listed}");
        let sessions: Vec<&Value> = listed["stages"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter(|summary| summary["arn"] == json!(arn))
                    .map(|summary| &summary["activeSessionId"])
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            sessions,
            vec![&json!("")],
            "case {case:?}: expected the summary to report an empty activeSessionId, \
             got {listed}"
        );
    }
}

/// A stage's endpoints are assigned to that stage: identical on every read of it, and
/// distinct between stages.
#[tokio::test]
async fn stage_endpoints_are_assigned_per_stage_and_do_not_change() {
    let emulator = Emulator::new();
    let mut whip_endpoints: BTreeSet<String> = BTreeSet::new();
    let stages = 4;

    for index in 0..stages {
        let name = format!("stage-{index}");
        let arn = emulator.create_stage(&json!({ "name": name })).await;
        let (status, first) = emulator
            .call("GetStage", &json!({ "arn": arn.clone() }))
            .await;
        assert_eq!(status, 200, "stage {name}: GetStage got {status} {first}");
        let (status, second) = emulator
            .call("GetStage", &json!({ "arn": arn.clone() }))
            .await;
        assert_eq!(status, 200, "stage {name}: GetStage got {status} {second}");
        assert_eq!(
            first["stage"]["endpoints"], second["stage"]["endpoints"],
            "stage {name}: expected two reads to report the same endpoints, got \
             {first} then {second}"
        );
        assert_stage_endpoints(&name, &first["stage"]);
        whip_endpoints.insert(
            first["stage"]["endpoints"]["whip"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    assert_eq!(
        whip_endpoints.len(),
        stages,
        "expected one distinct WHIP endpoint per stage, got {whip_endpoints:?}"
    );
}

/// Each member a supplied recording configuration leaves out is reported at the default
/// the model documents for it, by `CreateStage` and `UpdateStage` alike.
#[tokio::test]
async fn the_recording_configuration_fills_in_the_modeled_default_for_every_member_left_out() {
    let emulator = Emulator::new();
    // Every row that configures storage names a storage configuration of this service, so
    // it is created first and the rows name the ARN that create minted.
    let storage = emulator.create_storage_configuration("defaults").await;

    // Rows differ in which members the configuration supplies and in the values it
    // supplies for them: every `ParticipantRecordingMediaType` the model lists
    // (AUDIO_VIDEO, AUDIO_ONLY, NONE), both `ThumbnailRecordingMode`s (INTERVAL,
    // DISABLED), both `ThumbnailStorageType`s (SEQUENTIAL, LATEST), and each published
    // bound of the three numeric members — `targetIntervalSeconds` @range 1..86400,
    // `targetSegmentDurationSeconds` @range 2..10, `recordingReconnectWindowSeconds`
    // @range 0..300. From `smithy_query(query="shape",
    // shape="AutoParticipantRecordingConfiguration,ParticipantThumbnailConfiguration,
    // ParticipantRecordingHlsConfiguration,ParticipantRecordingMediaType,
    // ThumbnailRecordingMode,ThumbnailStorageType")`.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "storage configuration only",
            json!({ "storageConfigurationArn": storage.as_str() }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 0,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 60,
                    "storage": ["SEQUENTIAL"],
                    "recordingMode": "DISABLED",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
            }),
        ),
        (
            "audio only",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_ONLY"],
            }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_ONLY"],
                "recordingReconnectWindowSeconds": 0,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 60,
                    "storage": ["SEQUENTIAL"],
                    "recordingMode": "DISABLED",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
            }),
        ),
        (
            "no media recorded, thumbnails at the minimum interval",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["NONE"],
                "thumbnailConfiguration": {
                    "recordingMode": "INTERVAL",
                    "targetIntervalSeconds": 1,
                },
            }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["NONE"],
                "recordingReconnectWindowSeconds": 0,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 1,
                    "storage": ["SEQUENTIAL"],
                    "recordingMode": "INTERVAL",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
            }),
        ),
        (
            "thumbnails at the maximum interval, in both storage formats",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 86_400,
                    "storage": ["SEQUENTIAL", "LATEST"],
                },
            }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 0,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 86_400,
                    "storage": ["SEQUENTIAL", "LATEST"],
                    "recordingMode": "DISABLED",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
            }),
        ),
        (
            "HLS segments at the minimum duration, reconnect window at its minimum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "recordingReconnectWindowSeconds": 0,
                "hlsConfiguration": { "targetSegmentDurationSeconds": 2 },
            }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 0,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 60,
                    "storage": ["SEQUENTIAL"],
                    "recordingMode": "DISABLED",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 2 },
            }),
        ),
        (
            "every member supplied, at the maximum of each bound",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 300,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 86_400,
                    "storage": ["LATEST"],
                    "recordingMode": "INTERVAL",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 10 },
            }),
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 300,
                "recordParticipantReplicas": true,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 86_400,
                    "storage": ["LATEST"],
                    "recordingMode": "INTERVAL",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 10 },
            }),
        ),
        (
            // The documented reset: sending `""` returns every other member to default.
            "no storage configured, with other members supplied",
            json!({
                "storageConfigurationArn": "",
                "mediaTypes": ["AUDIO_ONLY"],
                "recordingReconnectWindowSeconds": 300,
                "thumbnailConfiguration": { "recordingMode": "INTERVAL" },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 10 },
            }),
            default_recording_configuration(),
        ),
    ];

    for (case, configuration, expected) in cases {
        let (status, created) = emulator
            .call(
                "CreateStage",
                &json!({ "autoParticipantRecordingConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage got {status} {created}"
        );
        assert_eq!(
            created["stage"]["autoParticipantRecordingConfiguration"], expected,
            "case {case:?}: expected CreateStage to report {expected}, got {created}"
        );
        let arn = created["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, read) = emulator.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: GetStage got {status} {read}");
        assert_eq!(
            read["stage"], created["stage"],
            "case {case:?}: expected GetStage to report the created stage, got {read}"
        );

        // The same configuration attached by an update reports the same members.
        let other = emulator.create_stage(&json!({})).await;
        let (status, updated) = emulator
            .call(
                "UpdateStage",
                &json!({
                    "arn": other,
                    "autoParticipantRecordingConfiguration": configuration,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStage got {status} {updated}"
        );
        assert_eq!(
            updated["stage"]["autoParticipantRecordingConfiguration"], expected,
            "case {case:?}: expected UpdateStage to report {expected}, got {updated}"
        );
    }

    // And one past each published bound is refused, so the bounds above are real    // bounds and not values this projection happens to accept.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a thumbnail interval one below the minimum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "thumbnailConfiguration": { "targetIntervalSeconds": 0 },
            }),
        ),
        (
            "a thumbnail interval one above the maximum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "thumbnailConfiguration": { "targetIntervalSeconds": 86_401 },
            }),
        ),
        (
            "an HLS segment duration one below the minimum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "hlsConfiguration": { "targetSegmentDurationSeconds": 1 },
            }),
        ),
        (
            "an HLS segment duration one above the maximum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "hlsConfiguration": { "targetSegmentDurationSeconds": 11 },
            }),
        ),
        (
            "a reconnect window one below the minimum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "recordingReconnectWindowSeconds": -1,
            }),
        ),
        (
            "a reconnect window one above the maximum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "recordingReconnectWindowSeconds": 301,
            }),
        ),
    ];

    for (case, configuration) in refused {
        let (status, response) = emulator
            .call(
                "CreateStage",
                &json!({ "autoParticipantRecordingConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: got {response}"
        );
    }
}

/// A participant token reports `capabilities` only when the REQUEST asked for some: an
/// explicitly empty list is honoured rather than replaced by the documented default, a
/// request that asks for nothing is answered without the member at all — the service does
/// not echo the default it applied — and a non-empty ask is reported as asked. The same
/// way `attributes` is carried only when the token holds attributes. Asserted on both
/// operations that issue a token.
#[tokio::test]
async fn a_participant_token_reports_a_collection_member_only_when_it_holds_entries() {
    let emulator = Emulator::new();
    let stage = emulator.create_stage(&json!({ "name": "tokens" })).await;

    // Rows differ in the capabilities the request asks for: absent (so the documented
    // `PUBLISH,SUBSCRIBE` default applies and nothing is reported), explicitly empty
    // (`@length 0..2` makes it legal), and
    // each non-empty subset of the two `ParticipantTokenCapability` values the model
    // lists, in both orders.
    let capability_cases: Vec<(&str, Value, Value)> = vec![
        ("absent", Value::Null, Value::Null),
        ("explicitly empty", json!([]), Value::Null),
        ("publish only", json!(["PUBLISH"]), json!(["PUBLISH"])),
        ("subscribe only", json!(["SUBSCRIBE"]), json!(["SUBSCRIBE"])),
        (
            "both, in the order asked for",
            json!(["SUBSCRIBE", "PUBLISH"]),
            json!(["SUBSCRIBE", "PUBLISH"]),
        ),
    ];

    for (case, requested, expected) in capability_cases {
        let mut request = json!({ "stageArn": stage });
        if !requested.is_null() {
            request["capabilities"] = requested.clone();
        }
        let (status, response) = emulator.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken got {status} {response}"
        );
        assert_eq!(
            response["participantToken"]["capabilities"], expected,
            "case {case:?}: expected capabilities {expected}, got {response}"
        );

        let mut configuration = json!({ "userId": "alice" });
        if !requested.is_null() {
            configuration["capabilities"] = requested.clone();
        }
        let (status, created) = emulator
            .call(
                "CreateStage",
                &json!({ "participantTokenConfigurations": [configuration] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage got {status} {created}"
        );
        assert_eq!(
            created["participantTokens"][0]["capabilities"], expected,
            "case {case:?}: expected CreateStage to report capabilities {expected}, got \
             {created}"
        );
    }

    // The same rule on `attributes`, the member this projection follows.
    let attribute_cases: Vec<(&str, Value, Value)> = vec![
        ("absent", Value::Null, Value::Null),
        ("explicitly empty", json!({}), Value::Null),
        (
            "one entry",
            json!({ "room": "main" }),
            json!({ "room": "main" }),
        ),
    ];

    for (case, requested, expected) in attribute_cases {
        let mut request = json!({ "stageArn": stage });
        if !requested.is_null() {
            request["attributes"] = requested;
        }
        let (status, response) = emulator.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "attributes case {case:?}: got {status} {response}"
        );
        assert_eq!(
            response["participantToken"]["attributes"], expected,
            "attributes case {case:?}: expected {expected}, got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Request claiming
// ---------------------------------------------------------------------------

/// This service signs as `ivs`, which `AmazonInteractiveVideoService` signs as too,
/// so it claims only the routes its OWN model carries. A path it does not model is
/// left to another service — here, with no other service registered, unclaimed.
#[tokio::test]
async fn the_service_claims_only_the_routes_its_model_carries() {
    let emulator = Emulator::new();
    // Rows: a route this model carries, and routes it does not (one belonging to
    // the sibling IVS service, one belonging to no service at all).
    let cases: Vec<(&str, &str, u16)> = vec![
        ("a route this model carries", "ListStages", 200),
        ("the sibling IVS service's route", "ListChannels", 404),
        ("a route no IVS service models", "ListUnicorns", 404),
    ];
    for (case, operation, expected_status) in cases {
        let (status, response) = emulator.call(operation, &json!({})).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({operation}): expected {expected_status}, got {status} \
             {response}"
        );
    }
}
