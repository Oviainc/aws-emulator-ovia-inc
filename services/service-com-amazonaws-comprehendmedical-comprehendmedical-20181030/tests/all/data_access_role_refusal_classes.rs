// SPDX-License-Identifier: CC0-1.0

//! The two ways a `DataAccessRoleArn` is refused travel under the SAME modelled error code,
//! so the MESSAGE is what tells them apart — and neither of them leaves a job behind.
//!
//! RULE: a `Start*Job` submission is checked against the `DataAccessRoleArn` member's
//! modelled shape FIRST and against this account's IAM records second; a value outside the
//! modelled shape is refused with the service's invalid-value tag, a well-formed value IAM
//! answers names no role this service can assume is refused with the trust sentence, both
//! carry `InvalidRequestException` and HTTP 400, and neither mints a `JobId` nor records a
//! job. A well-formed value IAM was not asked about — one naming another account — grounds
//! no refusal at all.
//!
//! The suite beside this one (`async_job_data_access_role.rs`) spans the FORMS of a trust
//! policy; this one spans the two refusal CLASSES together on one mounted emulator, which is
//! where the staging shows: a malformed ARN keeps reporting the invalid-value tag even with
//! IAM mounted and holding no such role, so the syntactic stage is the one that answers for
//! it.
//!
//! IAM is mounted beside this service on ONE engine, every role exists because IAM's own
//! `CreateRole` answered for it, and the account the well-formed rows name is read out of an
//! ARN IAM minted — so no row states an account id of its own.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The Region every case signs for, and the one the IAM client is addressed to.
const REGION: &str = "us-east-1";

/// The modelled error both refusal classes carry (`@httpError 400`).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The service principal a data-access role has to admit for a job to run under it.
const COMPREHEND_MEDICAL_PRINCIPAL: &str = "comprehendmedical.amazonaws.com";

/// A service principal that is NOT this service, for a role that trusts someone else.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// What the service reports for a value outside the member's modelled shape: correct the
/// string.
const ROLE_ARN_INVALID: &str =
    "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.";

/// What the service reports for a well-formed value naming a role it cannot assume: create
/// or re-trust the role. Fixed text — nothing of the request or of the store appears in it.
const NOT_TRUSTED: &str =
    "The provided IAM role does not trust Comprehend Medical service principle. See: \
     https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam";

/// This service mounted beside IAM — the owner of the roles it resolves — on ONE shared
/// engine, plus an IAM SDK client over the router's own in-process callback.
struct Emulator {
    router: MultiServiceRouter,
    iam: ::aws_sdk_iam::Client,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let iam = ::aws_sdk_iam::Client::from_conf(
            ::aws_sdk_iam::Config::builder()
                .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
                .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        Self { router, iam }
    }

    /// Create `role_name` with `document` as its trust policy, through IAM itself, and
    /// answer the ARN IAM minted for it.
    async fn create_role(&self, case: &str, role_name: &str, document: &str) -> String {
        self.iam
            .create_role()
            .role_name(role_name)
            .assume_role_policy_document(document)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup for case {case:?}: IAM must create {role_name}: {error:?}")
            })
            .role()
            .map(|role| role.arn().to_string())
            .unwrap_or_else(|| panic!("setup for case {case:?}: IAM must report an ARN"))
    }
}

/// Call `operation` with `body` over the wire this service speaks (awsJson1_1, `POST /` with
/// `X-Amz-Target`), signed for [`REGION`] so the handler resolves the request's Region the
/// way it does in production.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/comprehendmedical/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A trust policy with one statement, stating `effect` for the service `principal` on
/// `sts:AssumeRole`.
fn trust(effect: &str, principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The account field of an ARN IAM minted, so no row has to state an account id itself.
fn account_of(role_arn: &str) -> &str {
    role_arn
        .split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("IAM's own role ARN must carry an account field: {role_arn:?}"))
}

/// One job family whose start operation is implemented: the rule is about the member every
/// `Start*Job` declares, so every row is asserted through each of them. The other three
/// families (`StartICD10CMInferenceJob`, `StartRxNormInferenceJob`,
/// `StartSNOMEDCTInferenceJob`) are still scaffold stubs answering 501 and are left to the
/// issues that implement them.
struct Family {
    name: &'static str,
    start: &'static str,
    list: &'static str,
}

/// The families the rule is asserted through.
fn families() -> [Family; 2] {
    [
        Family {
            name: "PHI detection",
            start: "StartPHIDetectionJob",
            list: "ListPHIDetectionJobs",
        },
        Family {
            name: "entities detection v2",
            start: "StartEntitiesDetectionV2Job",
            list: "ListEntitiesDetectionV2Jobs",
        },
    ]
}

/// One `DataAccessRoleArn` value, and what the submission naming it reports: `None` for a
/// submission that is accepted, otherwise the whole message the refusal carries.
struct Row {
    case: String,
    role_arn: String,
    refused_with: Option<&'static str>,
}

/// The table the rule is asserted over: both refusal classes and the values that ground no
/// refusal, spanning how a value leaves the modelled shape (not an ARN, the wrong entity
/// type, an account field that is not twelve digits, an empty role name, each side of the
/// length bounds) and what IAM answers about a well-formed one (no such role, a role under a
/// path that is not there, a role trusting another service, a role denied this service, a
/// role trusting this service, another account's role).
async fn rows(emulator: &Emulator) -> Vec<Row> {
    let trusting = emulator
        .create_role(
            "a role trusting this service",
            "cm-refusal-classes-trusting",
            &trust("Allow", COMPREHEND_MEDICAL_PRINCIPAL),
        )
        .await;
    let other = emulator
        .create_role(
            "a role trusting another service",
            "cm-refusal-classes-other",
            &trust("Allow", OTHER_PRINCIPAL),
        )
        .await;
    let denied = emulator
        .create_role(
            "a role denying this service",
            "cm-refusal-classes-denied",
            &trust("Deny", COMPREHEND_MEDICAL_PRINCIPAL),
        )
        .await;
    // The account every well-formed row names is the one IAM itself minted the roles in.
    let account = account_of(&trusting).to_string();

    vec![
        // Outside the modelled shape: @pattern
        // ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$ and @length 20..2048 on `IamRoleArn`
        // (smithy_query(query="shape",shape="IamRoleArn")).
        Row {
            case: "a value that is not an ARN at all".to_string(),
            role_arn: "not-a-valid-arn".to_string(),
            refused_with: Some(ROLE_ARN_INVALID),
        },
        Row {
            case: "an ARN naming another IAM entity type of this account".to_string(),
            role_arn: format!("arn:aws:iam::{account}:user/cm-refusal-classes-trusting"),
            refused_with: Some(ROLE_ARN_INVALID),
        },
        Row {
            case: "an ARN whose account field is not twelve digits".to_string(),
            role_arn: "arn:aws:iam::12345:role/cm-refusal-classes-trusting".to_string(),
            refused_with: Some(ROLE_ARN_INVALID),
        },
        Row {
            case: "an ARN naming an empty role".to_string(),
            role_arn: format!("arn:aws:iam::{account}:role/"),
            refused_with: Some(ROLE_ARN_INVALID),
        },
        Row {
            case: "a value one character below the length minimum".to_string(),
            role_arn: "a".repeat(19),
            refused_with: Some(ROLE_ARN_INVALID),
        },
        Row {
            case: "an ARN one character past the length maximum".to_string(),
            role_arn: {
                let prefix = format!("arn:aws:iam::{account}:role/");
                let fill = 2049 - prefix.chars().count();
                format!("{prefix}{}", "r".repeat(fill))
            },
            refused_with: Some(ROLE_ARN_INVALID),
        },
        // Well formed, and IAM answers that the role cannot be assumed.
        Row {
            case: "a well-formed ARN naming a role this account does not hold".to_string(),
            role_arn: format!("arn:aws:iam::{account}:role/cm-refusal-classes-absent"),
            refused_with: Some(NOT_TRUSTED),
        },
        Row {
            case: "a role this account does not hold, under a path".to_string(),
            role_arn: format!("arn:aws:iam::{account}:role/service-role/cm-refusal-classes-absent"),
            refused_with: Some(NOT_TRUSTED),
        },
        Row {
            case: "a role this account holds that trusts another service".to_string(),
            role_arn: other,
            refused_with: Some(NOT_TRUSTED),
        },
        Row {
            case: "a role this account holds that denies this service".to_string(),
            role_arn: denied,
            refused_with: Some(NOT_TRUSTED),
        },
        // Well formed, and nothing IAM answered bars the submission.
        Row {
            case: "a role this account holds that trusts this service".to_string(),
            role_arn: trusting,
            refused_with: None,
        },
        Row {
            case: "another account's role, which this emulator's IAM is not asked about"
                .to_string(),
            role_arn: "arn:aws:iam::123456789012:role/cm-refusal-classes-cross-account".to_string(),
            refused_with: None,
        },
    ]
}

/// The `__type` an awsJson1_1 error body carries.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message an answer carries, whichever casing its envelope spells the member with: the
/// boundary refusal renders the protocol's own `message`, a modelled exception the `Message`
/// member the model declares.
fn error_message(answer: &Value) -> Option<&str> {
    answer["message"]
        .as_str()
        .or_else(|| answer["Message"].as_str())
}

/// The jobs `operation` reports for the job name `job_name`.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    job_name: &str,
) -> Vec<Value> {
    let filter = json!({ "Filter": { "JobName": job_name } });
    let (status, answer) = call(router, operation, &filter).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {answer}"
    );
    answer["ComprehendMedicalAsyncJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_two_data_access_role_refusals_share_a_code_and_differ_only_in_the_message() {
    let emulator = Emulator::new();
    let rows = rows(&emulator).await;

    for family in families() {
        for (index, row) in rows.iter().enumerate() {
            let case = format!("{} with {}", family.name, row.case);
            let job_name = format!("classes-{}-{index}", family.start);
            let (status, answer) = call(
                &emulator.router,
                family.start,
                &json!({
                    "JobName": job_name,
                    "InputDataConfig": { "S3Bucket": "in-bucket", "S3Key": "input/doc.txt" },
                    "OutputDataConfig": { "S3Bucket": "out-bucket", "S3Key": "output/" },
                    "DataAccessRoleArn": row.role_arn,
                    "LanguageCode": "en",
                }),
            )
            .await;

            let Some(expected) = row.refused_with else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: nothing bars this submission, so it must start a job, got \
                     {status} {answer}"
                );
                assert!(
                    answer["JobId"].as_str().is_some(),
                    "case {case:?}: expected a JobId, got {answer}"
                );
                let page = listed(&emulator.router, &case, family.list, &job_name).await;
                assert_eq!(
                    page.len(),
                    1,
                    "case {case:?}: expected the accepted submission to be listed once, got \
                     {page:?}"
                );
                continue;
            };

            // Both classes travel under ONE code and status: a caller branching on the code
            // alone cannot tell them apart, which is why the message is asserted whole.
            let code = error_code(&answer);
            assert_eq!(
                (status, code.as_str()),
                (400, INVALID_REQUEST),
                "case {case:?}: expected 400 {INVALID_REQUEST}, got {status} {code:?}: {answer}"
            );
            assert_eq!(
                error_message(&answer),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {answer}"
            );
            assert!(
                answer.get("JobId").is_none(),
                "case {case:?}: a refused submission mints no JobId, got {answer}"
            );

            // Nothing was persisted, so the family lists no job for the refused submission.
            let page = listed(&emulator.router, &case, family.list, &job_name).await;
            assert!(
                page.is_empty(),
                "case {case:?}: a refused submission must leave no listable job, got {page:?}"
            );
        }
    }

    // The two classes really are two: the sentences differ, and neither carries anything of
    // the request — no account id, no role name, no job name, no bucket.
    assert_ne!(
        ROLE_ARN_INVALID, NOT_TRUSTED,
        "the two refusal classes must be told apart by their messages"
    );
    for row in &rows {
        if row.refused_with != Some(NOT_TRUSTED) {
            continue;
        }
        let name = row.role_arn.rsplit('/').next().unwrap_or_default();
        assert!(
            !NOT_TRUSTED.contains(&row.role_arn) && !NOT_TRUSTED.contains(name),
            "case {:?}: the trust refusal must echo nothing of the request",
            row.case
        );
    }
}

/// RULE (the bound the first stage answers for): the member's modelled `@length 20..2048`
/// is INCLUSIVE, so the invalid-value tag answers for a value OUTSIDE it and never for one
/// at either end — a value at a bound is inside the modelled shape, so whatever happens to
/// it next is the resolution stage's answer and not this tag.
///
/// The bounds are the model's own
/// (`smithy_query(query="shape",shape="IamRoleArn")`: `@length 20..2048`,
/// `@pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`). The two constraints interact at
/// the low end: no value of 20 characters can also spell the pattern, so the shortest value
/// the member admits is a whole ARN well above that bound.
#[tokio::test]
async fn the_modelled_length_bounds_of_the_role_arn_are_inclusive() {
    let emulator = Emulator::new();
    // The ARN whose role name grows to put the value at a given total length. The account
    // field is twelve digits, as the pattern requires.
    let arn_of_length = |total: usize| {
        let prefix = "arn:aws:iam::123456789012:role/";
        format!("{prefix}{}", "r".repeat(total - prefix.chars().count()))
    };

    // (case, the value, whether the invalid-value tag answers for it)
    let cases: Vec<(&str, String, bool)> = vec![
        ("one character below the length minimum", "a".repeat(19), true),
        // At the minimum, and still outside the pattern: 20 characters cannot spell an ARN
        // of this shape, so the first stage answers for it anyway.
        ("a value at the length minimum", "a".repeat(20), true),
        (
            "the shortest value that spells the pattern",
            "arn:aws:iam::123456789012:role/r".to_string(),
            false,
        ),
        ("an ARN at the length maximum", arn_of_length(2048), false),
        (
            "an ARN one character past the length maximum",
            arn_of_length(2049),
            true,
        ),
    ];

    for family in families() {
        for (name, value, tagged) in &cases {
            let case = format!("{} with {name}", family.name);
            let (status, answer) = call(
                &emulator.router,
                family.start,
                &json!({
                    "JobName": "bounds-probe",
                    "InputDataConfig": { "S3Bucket": "in-bucket", "S3Key": "input/doc.txt" },
                    "OutputDataConfig": { "S3Bucket": "out-bucket", "S3Key": "output/" },
                    "DataAccessRoleArn": value,
                    "LanguageCode": "en",
                }),
            )
            .await;
            let message = error_message(&answer).unwrap_or_default();
            if *tagged {
                let code = error_code(&answer);
                assert_eq!(
                    (status, code.as_str(), message),
                    (400, INVALID_REQUEST, ROLE_ARN_INVALID),
                    "case {case:?}: expected the invalid-value refusal, got {status} \
                     {code:?}: {answer}"
                );
            } else {
                assert_ne!(
                    message, ROLE_ARN_INVALID,
                    "case {case:?}: a value inside the modelled bounds must not be refused as \
                     malformed, got {status} {answer}"
                );
            }
        }
    }
}
