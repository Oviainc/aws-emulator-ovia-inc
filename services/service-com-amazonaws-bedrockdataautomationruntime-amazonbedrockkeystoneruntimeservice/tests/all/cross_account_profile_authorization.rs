// SPDX-License-Identifier: CC0-1.0

//! A submission is AUTHORIZED against the data-automation profile it names.
//!
//! The RULE asserted here: a submission runs under the profile its
//! `dataAutomationProfileArn` names, so an ARN whose account component names an account
//! other than the caller's own is refused with the modelled `AccessDeniedException`
//! (HTTP 403, `smithy_query` shape `AccessDeniedException`) carrying AWS's denial
//! sentence for the CALLER — fixed text `User: `, the caller's own account id, fixed text
//! ` is not authorized to access this resource` — and NOTHING is minted. The modelled
//! pattern (`smithy_query` shape `DataAutomationProfileArn`) admits
//! `(aws|[0-9]{12})` in that field, so a foreign twelve-digit account is well formed and
//! the refusal is an authorization one rather than a constraint violation; the literal
//! `aws` marks a profile AWS publishes for every account, which no caller is foreign to.
//!
//! The rows span what the rule generalises over: WHICH account the ARN names (the
//! caller's own, AWS's own, four different foreign accounts — including one a single digit
//! away from the caller's), the Region the ARN carries, and the profile suffix under it.
//! The accepted rows are the positive control: they mint a job that resolves through
//! `GetDataAutomationStatus`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The account this emulator serves — the canonical placeholder account every ARN it
/// mints carries, and the account a caller's credentials speak for. Spelled out here so
/// the expected denial sentence is pinned rather than read from the crate.
const CALLER_ACCOUNT: &str = "000000000000";

/// The denial AWS answers an unauthorized caller with: two fixed clauses around the
/// CALLER's own account id. The foreign account the request carried is NOT in it.
fn expected_denial() -> String {
    format!("User: {CALLER_ACCOUNT} is not authorized to access this resource")
}

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn`, arranged in the owning
/// service's store by [`service`]: what tells the service WHAT to extract, which every
/// submission has to name.
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/profiles";

/// One row: the case, the `dataAutomationProfileArn` to submit, and whether the
/// submission must be accepted.
struct ProfileCase {
    case: &'static str,
    profile_arn: &'static str,
    accepted: bool,
}

/// The table. Every ARN here satisfies the modelled `DataAutomationProfileArn` pattern,
/// so what decides each row is authorization and nothing else.
const ROWS: &[ProfileCase] = &[
    ProfileCase {
        case: "the caller's own account",
        profile_arn:
            "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/us.data-automation-v1",
        accepted: true,
    },
    ProfileCase {
        case: "the caller's own account in another Region",
        profile_arn:
            "arn:aws:bedrock:eu-west-1:000000000000:data-automation-profile/eu.data-automation-v1",
        accepted: true,
    },
    ProfileCase {
        case: "AWS's own service-owned profile",
        profile_arn:
            "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1",
        accepted: true,
    },
    ProfileCase {
        case: "a foreign account",
        profile_arn:
            "arn:aws:bedrock:us-west-2:111122223333:data-automation-profile/us.data-automation-v1",
        accepted: false,
    },
    ProfileCase {
        case: "another foreign account",
        profile_arn:
            "arn:aws:bedrock:us-west-2:210987654321:data-automation-profile/us.data-automation-v1",
        accepted: false,
    },
    ProfileCase {
        case: "a foreign account one digit away from the caller's",
        profile_arn:
            "arn:aws:bedrock:us-west-2:000000000001:data-automation-profile/us.data-automation-v1",
        accepted: false,
    },
    ProfileCase {
        case: "a foreign account in another Region, under another profile suffix",
        profile_arn:
            "arn:aws:bedrock:ap-southeast-2:123456789012:data-automation-profile/apac.data-automation-v1",
        accepted: false,
    },
    ProfileCase {
        case: "a foreign account under a GovCloud partition profile",
        profile_arn:
            "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:data-automation-profile/us-gov.data-automation-v1",
        accepted: false,
    },
];

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }

    /// The error message the body carries, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// A router serving only this service over one shared engine, and that engine — so a test
/// can read how many invocations the store holds.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "profile-authorization-projects");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// How many invocations this service's store holds.
fn invocation_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    bda::state::service_state(&**engine)
        .expect("this service's store on this engine")
        .lock()
        .invocation_count()
}

/// Call one operation of this service the way an SDK would (awsJson1_1: `POST /` with
/// `X-Amz-Target`).
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

#[tokio::test]
async fn a_profile_arn_of_another_account_is_refused_and_mints_nothing() {
    let (router, engine) = service();

    for row in ROWS {
        let case = row.case;
        let before = invocation_count(&engine);
        let answer = call(
            &router,
            "InvokeDataAutomationAsync",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/authorized/"},
                "dataAutomationProfileArn": row.profile_arn,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            }),
        )
        .await;

        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the submission to be accepted, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer.body["invocationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
                .to_string();
            assert_eq!(
                invocation_count(&engine),
                before + 1,
                "case {case:?}: an accepted submission must persist exactly one invocation"
            );
            // The positive control resolves: the accepted job is a job a reader answers
            // for.
            let read = call(
                &router,
                "GetDataAutomationStatus",
                json!({"invocationArn": arn}),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the accepted job must resolve, got {} / {}",
                read.status, read.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected_denial(),
            "case {case:?}: expected AWS's denial sentence, got {:?}",
            answer.message()
        );
        // The refusal names the CALLER, never the account the request carried: a caller
        // must not learn from it what exists in an account that is not its own.
        let foreign_account = row
            .profile_arn
            .split(':')
            .nth(4)
            .expect("the ARN's account segment");
        assert!(
            !answer.message().contains(foreign_account)
                && !answer.message().contains(row.profile_arn),
            "case {case:?}: the denial must not echo {foreign_account:?} or the ARN, got {:?}",
            answer.message()
        );
        assert_eq!(
            invocation_count(&engine),
            before,
            "case {case:?}: a denied submission must mint nothing"
        );
    }
}

#[tokio::test]
async fn a_denied_submission_is_refused_before_every_other_rule() {
    // Authorization is decided from the ARN's account field alone, so it does not depend
    // on anything else about the request being right: a submission that is ALSO wrong in
    // another way still earns the denial, and still mints nothing.
    let (router, engine) = service();
    let foreign =
        "arn:aws:bedrock:us-west-2:111122223333:data-automation-profile/us.data-automation-v1";

    let rows: &[(&str, Value)] = &[
        (
            "naming no extraction target at all",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/denied/"},
                "dataAutomationProfileArn": foreign,
            }),
        ),
        (
            "naming a project no store holds",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/denied/"},
                "dataAutomationProfileArn": foreign,
                "dataAutomationConfiguration": {
                    "dataAutomationProjectArn":
                        "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/absent",
                },
            }),
        ),
        (
            "carrying a tag key the modelled pattern reserves",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/denied/"},
                "dataAutomationProfileArn": foreign,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
                "tags": [{"key": "aws:reserved", "value": "x"}],
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = call(&router, "InvokeDataAutomationAsync", body.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected the authorization refusal, got {} / {}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected_denial(),
            "case {case:?}: expected AWS's denial sentence, got {:?}",
            answer.message()
        );
    }

    assert_eq!(
        invocation_count(&engine),
        0,
        "no denied submission may persist an invocation"
    );
}
