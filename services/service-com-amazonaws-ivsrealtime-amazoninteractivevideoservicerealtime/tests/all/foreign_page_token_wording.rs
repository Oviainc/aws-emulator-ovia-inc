// SPDX-License-Identifier: CC0-1.0

//! One wording for a page token a listing did not issue, across EVERY listing of this
//! service.
//!
//! The rule: a listing refuses a `nextToken` it did not issue with the modelled
//! `ValidationException` (HTTP 400) whose text names the operation, the member and the
//! requirement — `<Operation>.nextToken is required to be a pagination token issued by
//! <Operation>` — and the empty token is the first page, not a refusal.
//!
//! Every listing of this service builds that sentence from the one place the service
//! words it, so no listing can word it differently from the others. This suite drives the
//! shared path through each of them, including the two listings whose own issued tokens
//! are foreign to each other (`ListStages` and `ListStorageConfigurations`), so a change
//! to the wording that reaches only some of the listings fails here.
//!
//! Rows differ along what the rule generalises over: the listing (so the operation name
//! in the sentence), and HOW the token fails to name a position in that listing (not
//! base64, base64 of text no listing issued, base64 of a marker with no position, and a
//! token another listing of this same service really did issue).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries, and so the account every ARN
/// built by hand here carries too (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// A `StageSessionId` of the modelled form: `@length(16, 16)` with `@pattern
/// ^st-[a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation",operation="ListParticipants")` — member `sessionId`.
const SESSION_ID: &str = "st-abcdef123456j";

/// A `ParticipantId` of the modelled form (`@pattern ^[a-zA-Z0-9-_]*$`), from
/// `smithy_query(query="operation",operation="ListParticipantEvents")` — member
/// `participantId`.
const PARTICIPANT_ID: &str = "participant-1";

/// The sentence every listing of this service refuses a token it did not issue with.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
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
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A page of `operation`, asserting the call succeeded, and the `nextToken` it
    /// carried: a token this listing really did issue.
    async fn issued_token(&self, operation: &str, request: &Value) -> String {
        let (status, response) = self.call(operation, request).await;
        assert_eq!(
            status, 200,
            "fixture {operation} {request}: expected HTTP 200, got {status} {response}"
        );
        response["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "fixture {operation} {request}: expected a nextToken while records \
                     remain, got {response}"
                )
            })
            .to_string()
    }
}

/// The error code a refusal states, or `"<absent>"` so a failing assertion names the
/// missing member rather than panicking inside the reader.
fn error_code(response: &Value) -> &str {
    response
        .get("Code")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// One listing of this service, with the members a call to it must carry besides
/// `nextToken`.
struct Listing {
    operation: &'static str,
    required: Value,
}

/// Every listing this service implements, with the members each one requires. The
/// required members come from `smithy_query(query="operation",operation="ListStages,
/// ListStorageConfigurations,ListEncoderConfigurations,ListIngestConfigurations,
/// ListCompositions,ListStageSessions,ListParticipants,ListParticipantEvents,
/// ListParticipantReplicas,ListPublicKeys")`.
fn listings(stage_arn: &str) -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListStages",
            required: json!({}),
        },
        Listing {
            operation: "ListStorageConfigurations",
            required: json!({}),
        },
        Listing {
            operation: "ListEncoderConfigurations",
            required: json!({}),
        },
        Listing {
            operation: "ListIngestConfigurations",
            required: json!({}),
        },
        Listing {
            operation: "ListCompositions",
            required: json!({}),
        },
        Listing {
            operation: "ListStageSessions",
            required: json!({ "stageArn": stage_arn }),
        },
        Listing {
            operation: "ListParticipants",
            required: json!({ "stageArn": stage_arn, "sessionId": SESSION_ID }),
        },
        Listing {
            operation: "ListParticipantEvents",
            required: json!({
                "stageArn": stage_arn,
                "sessionId": SESSION_ID,
                "participantId": PARTICIPANT_ID,
            }),
        },
        Listing {
            operation: "ListParticipantReplicas",
            required: json!({
                "sourceStageArn": stage_arn,
                "participantId": PARTICIPANT_ID,
            }),
        },
        Listing {
            operation: "ListPublicKeys",
            required: json!({}),
        },
    ]
}

/// A call to `listing` carrying `token` as its `nextToken`.
fn request_with_token(listing: &Listing, token: &str) -> Value {
    let mut request = listing.required.clone();
    request["nextToken"] = json!(token);
    request
}

/// Every listing of this service refuses a token it did not issue in the SAME words,
/// naming its own operation — including a token another listing of this service issued.
#[tokio::test]
async fn every_listing_refuses_a_foreign_page_token_in_the_same_words() {
    let emulator = RealTime::new();

    // Two stages, so `ListStages` has a page left over to issue a token for, and one of
    // them is a real stage the session-scoped listings can name.
    let mut stage_arn = String::new();
    for index in 0..2 {
        let (status, response) = emulator
            .call("CreateStage", &json!({ "name": format!("stage-{index}") }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStage: expected HTTP 200, got {status} {response}"
        );
        if index == 0 {
            stage_arn = response["stage"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }
    assert!(
        stage_arn.starts_with(&format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/")),
        "fixture CreateStage: expected a minted stage ARN, got {stage_arn:?}"
    );

    // Two storage configurations, so `ListStorageConfigurations` issues a token too: the
    // two listings' tokens are foreign to each other.
    for index in 0..2 {
        let (status, response) = emulator
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": format!("bucket-{index}") } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStorageConfiguration: expected HTTP 200, got {status} {response}"
        );
    }

    let stage_token = emulator
        .issued_token("ListStages", &json!({ "maxResults": 1 }))
        .await;
    let storage_token = emulator
        .issued_token("ListStorageConfigurations", &json!({ "maxResults": 1 }))
        .await;

    // How the token fails to name a position in the listing it is offered to. The last
    // two rows are tokens a listing of this service really issued, so they are refused
    // only because they name a position among records these pages do not hold.
    let malformed: Vec<(&str, String)> = vec![
        ("not base64 at all", "++++".to_string()),
        (
            "base64 of text no listing issued",
            "c29tZXRoaW5nLWVsc2U=".to_string(),
        ),
        (
            "base64 of a marker with a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0b3JhZ2UtY29uZmlndXJhdGlvbi1hZnRlcjpub3Bl".to_string(),
        ),
    ];
    let issued_elsewhere: Vec<(&str, &str, String)> = vec![
        ("a token issued by ListStages", "ListStages", stage_token),
        (
            "a token issued by ListStorageConfigurations",
            "ListStorageConfigurations",
            storage_token,
        ),
    ];

    for listing in listings(&stage_arn) {
        let expected = foreign_page_token_message(listing.operation);
        let mut tokens: Vec<(String, String)> = malformed
            .iter()
            .map(|(case, token)| ((*case).to_string(), token.clone()))
            .collect();
        tokens.extend(
            issued_elsewhere
                .iter()
                // A listing's OWN token is not foreign to it, so it is not a row here.
                .filter(|(_, issuer, _)| *issuer != listing.operation)
                .map(|(case, _, token)| ((*case).to_string(), token.clone())),
        );

        for (how, token) in tokens {
            let case = format!("{} / {how}", listing.operation);
            let (status, response) = emulator
                .call(listing.operation, &request_with_token(&listing, &token))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected,
                "case {case:?}: expected the one foreign-token sentence {expected:?}, got {}",
                error_message(&response)
            );
        }
    }
}

/// The other half of the rule: the empty token is the first page, and a token a listing
/// DID issue resumes it — so the refusal above is about the token, not about any token.
#[tokio::test]
async fn an_absent_or_empty_token_is_the_first_page_and_an_issued_token_resumes() {
    let emulator = RealTime::new();

    // Three of each kind, so a first page of one leaves two behind.
    for index in 0..3 {
        let (status, response) = emulator
            .call("CreateStage", &json!({ "name": format!("paged-{index}") }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStage: expected HTTP 200, got {status} {response}"
        );
        let (status, response) = emulator
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": format!("paged-{index}") } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStorageConfiguration: expected HTTP 200, got {status} {response}"
        );
    }

    // Rows differ along the listing, and over the two forms of "start at the
    // beginning": the member absent, and the empty token.
    let cases: Vec<(&str, &str)> = vec![
        ("ListStages", "stages"),
        ("ListStorageConfigurations", "storageConfigurations"),
    ];
    for (operation, member) in cases {
        for (how, request) in [
            ("nextToken absent", json!({})),
            ("the empty token", json!({ "nextToken": "" })),
        ] {
            let case = format!("{operation} / {how}");
            let (status, response) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first page is not a refusal, got {status} {response}"
            );
            assert_eq!(
                response[member].as_array().map(Vec::len).unwrap_or_default(),
                3,
                "case {case:?}: expected every record on the first page, got {response}"
            );
        }

        // The token this listing issued resumes where its page ended: the records it
        // reports are the ones the first page did not.
        let first = emulator
            .issued_token(operation, &json!({ "maxResults": 1 }))
            .await;
        let (status, response) = emulator
            .call(operation, &json!({ "nextToken": first }))
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: its own token must resume the listing, got {status} {response}"
        );
        assert_eq!(
            response[member].as_array().map(Vec::len).unwrap_or_default(),
            2,
            "case {operation:?}: expected the two records the first page left, got {response}"
        );
    }
}
