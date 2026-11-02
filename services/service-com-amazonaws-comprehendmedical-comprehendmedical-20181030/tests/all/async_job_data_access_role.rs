// SPDX-License-Identifier: CC0-1.0

//! The data-access role an asynchronous job is submitted with is resolved against the
//! account's IAM records BEFORE the job is minted.
//!
//! RULE: a `Start*Job` operation resolves the `DataAccessRoleArn` it is given against this
//! account's IAM records at submit time and, when IAM answers that the account holds no
//! such role or that the role's trust policy does not let this service assume it, refuses
//! the submission with the modelled `InvalidRequestException` (HTTP 400) — minting no
//! `JobId` and recording no job, so the family's list and describe operations report
//! nothing for it. A value this account's IAM was not asked about, because it names no role
//! of THIS account, grounds no refusal.
//!
//! The rows span what the rule generalises over: the FORM of the trust policy (which
//! service principal it names, whether it names several, which statement carries the grant,
//! whether the effect is an `Allow`, whether an explicit `Deny` stands beside it, which
//! action it names and which wildcard stands in that action's place), whether this
//! account's IAM holds the named role at all, and which path it holds it under — and every
//! row is asserted through BOTH of the job families this crate implements, since the rule is
//! about the member and not about one operation.
//!
//! IAM is mounted beside this service on ONE engine and every role is created through IAM's
//! own `CreateRole`, so a role exists only because IAM answered for it and the resolution
//! under test runs over the same in-process callback as in production.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The Region every case signs for, and the one the IAM client is addressed to.
const REGION: &str = "us-east-1";

/// The service principal a data-access role has to admit for a job to run under it.
const COMPREHEND_MEDICAL_PRINCIPAL: &str = "comprehendmedical.amazonaws.com";

/// A service principal that is NOT this service, for the roles that trust someone else.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The modelled error a refused submission carries (`@httpError 400`).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// What the refusal reports, as the service words it for a role it cannot assume. Fixed
/// text: the failure is about the trust relationship, not about any value the request
/// carried.
const NOT_TRUSTED: &str =
    "The provided IAM role does not trust Comprehend Medical service principle. See: \
     https://docs.aws.amazon.com/comprehend-medical/latest/dev/textanalysis-batchapi.html#batch-iam";

/// This service mounted beside IAM — the owner of the roles it resolves — on ONE shared
/// engine, plus an IAM SDK client over the router's own in-process callback: the very seam
/// this service's own resolution reaches IAM through.
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

    /// Call `operation` with `body` through this emulator's router.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        call(&self.router, operation, body).await
    }

    /// Create `role_name` under `path` with `document` as its trust policy, through IAM
    /// itself, and answer the ARN IAM minted for it.
    async fn create_role(&self, case: &str, path: &str, role_name: &str, document: &str) -> String {
        self.iam
            .create_role()
            .role_name(role_name)
            .path(path)
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

/// Call `operation` with `body` over the wire this service speaks (awsJson1_1, `POST /`
/// with `X-Amz-Target`), signed for [`REGION`] so the handler resolves the request's Region
/// the way it does in production.
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

/// A trust policy with one statement, stating `effect` for `principal` on `action`.
fn trust(effect: &str, action: Value, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": action,
        }],
    })
    .to_string()
}

/// One job family this crate implements: its start, list and describe operations, which
/// every row is asserted through.
struct Family {
    /// What the family is called in a case name.
    name: &'static str,
    start: &'static str,
    list: &'static str,
    describe: &'static str,
}

/// The families the rule covers — the two whose start operation is implemented. The other
/// three (`StartICD10CMInferenceJob`, `StartRxNormInferenceJob`,
/// `StartSNOMEDCTInferenceJob`) are still scaffold stubs that answer 501, so they resolve
/// nothing and are left to the issues that implement them.
fn families() -> [Family; 2] {
    [
        Family {
            name: "PHI detection",
            start: "StartPHIDetectionJob",
            list: "ListPHIDetectionJobs",
            describe: "DescribePHIDetectionJob",
        },
        Family {
            name: "entities detection v2",
            start: "StartEntitiesDetectionV2Job",
            list: "ListEntitiesDetectionV2Jobs",
            describe: "DescribeEntitiesDetectionV2Job",
        },
    ]
}

/// One role value, and whether a submission naming it is accepted.
struct Row {
    case: &'static str,
    role_arn: String,
    accepted: bool,
}

/// The table the rule is asserted over, built from roles IAM actually holds.
async fn rows(emulator: &Emulator) -> Vec<Row> {
    let mut rows = Vec::new();

    // (case, the role's IAM path, its trust document, whether the submission is accepted)
    let held: [(&str, &str, String, bool); 9] = [
        (
            "a role trusting this service, named as a string",
            "/",
            trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": COMPREHEND_MEDICAL_PRINCIPAL }),
            ),
            true,
        ),
        (
            "the same trust, under an IAM path",
            "/service-role/",
            trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": COMPREHEND_MEDICAL_PRINCIPAL }),
            ),
            true,
        ),
        (
            "this service among several, both members named as lists",
            "/",
            trust(
                "Allow",
                json!(["sts:TagSession", "sts:AssumeRole"]),
                json!({ "Service": [OTHER_PRINCIPAL, COMPREHEND_MEDICAL_PRINCIPAL] }),
            ),
            true,
        ),
        (
            "this service admitted through a service wildcard in the action",
            "/",
            trust(
                "Allow",
                json!("sts:*"),
                json!({ "Service": COMPREHEND_MEDICAL_PRINCIPAL }),
            ),
            true,
        ),
        (
            "the grant carried by the second statement",
            "/",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": OTHER_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": COMPREHEND_MEDICAL_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                ],
            })
            .to_string(),
            true,
        ),
        (
            // The recorded negative: a role that exists and trusts another service.
            "a role trusting another service only",
            "/",
            trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": OTHER_PRINCIPAL }),
            ),
            false,
        ),
        (
            "a role whose statement denies this service",
            "/",
            trust(
                "Deny",
                json!("sts:AssumeRole"),
                json!({ "Service": COMPREHEND_MEDICAL_PRINCIPAL }),
            ),
            false,
        ),
        (
            // IAM's deny precedence: an explicit `Deny` wins over the `Allow` beside it.
            "a role that both allows and denies this service",
            "/",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": COMPREHEND_MEDICAL_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": COMPREHEND_MEDICAL_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                ],
            })
            .to_string(),
            false,
        ),
        (
            // A trust policy governs `sts:AssumeRole`; naming this service for some other
            // action grants it nothing.
            "a role allowing this service an action that is not assuming it",
            "/",
            trust(
                "Allow",
                json!("s3:GetObject"),
                json!({ "Service": COMPREHEND_MEDICAL_PRINCIPAL }),
            ),
            false,
        ),
    ];

    for (index, (case, path, document, accepted)) in held.into_iter().enumerate() {
        let role_arn = emulator
            .create_role(case, path, &format!("cm-data-access-{index}"), &document)
            .await;
        rows.push(Row {
            case,
            role_arn,
            accepted,
        });
    }

    // The values IAM holds nothing for. The first is the recorded case: an ARN of this
    // account, well formed under the member's modelled pattern, naming a role that is not
    // there. The last two name ANOTHER account, whose IAM this emulator cannot answer for,
    // so nothing grounds a refusal.
    rows.push(Row {
        case: "a well-formed ARN naming a role this account does not hold",
        role_arn: format!(
            "arn:aws:iam::{}:role/cm-data-access-absent",
            ::engine::emulator_account_id()
        ),
        accepted: false,
    });
    rows.push(Row {
        case: "a role this account does not hold, under a path",
        role_arn: format!(
            "arn:aws:iam::{}:role/service-role/cm-data-access-absent",
            ::engine::emulator_account_id()
        ),
        accepted: false,
    });
    rows.push(Row {
        case: "another account's role",
        role_arn: "arn:aws:iam::123456789012:role/cm-cross-account".to_string(),
        accepted: true,
    });
    rows.push(Row {
        case: "another partition's role in another account",
        role_arn: "arn:aws-us-gov:iam::123456789012:role/cm-cross-account".to_string(),
        accepted: true,
    });

    rows
}

/// The `__type` an awsJson1_1 error body carries.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
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
async fn a_start_resolves_its_data_access_role_before_it_mints_a_job() {
    let emulator = Emulator::new();
    let rows = rows(&emulator).await;

    for family in families() {
        for (index, row) in rows.iter().enumerate() {
            let case = format!("{} with {}", family.name, row.case);
            let job_name = format!("probe-{}-{index}", family.start);
            let (status, answer) = emulator
                .call(
                    family.start,
                    &json!({
                        "JobName": job_name,
                        "InputDataConfig": { "S3Bucket": "in-bucket", "S3Key": "docs/" },
                        "OutputDataConfig": { "S3Bucket": "out-bucket", "S3Key": "out/" },
                        "DataAccessRoleArn": row.role_arn,
                        "LanguageCode": "en",
                    }),
                )
                .await;

            if row.accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a role nothing IAM answered bars must start the job, \
                     got {status} {answer}"
                );
                let job_id = answer["JobId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {answer}"))
                    .to_string();

                // ROUND TRIP: the accepted submission is the job the family holds, with
                // the role it was given.
                let (status, described) = emulator
                    .call(family.describe, &json!({ "JobId": job_id }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the started job to read back, got \
                     {status} {described}"
                );
                let read_role = described
                    .pointer("/ComprehendMedicalAsyncJobProperties/DataAccessRoleArn")
                    .and_then(Value::as_str);
                assert_eq!(
                    read_role,
                    Some(row.role_arn.as_str()),
                    "case {case:?}: expected the stored role to be {}, got {described}",
                    row.role_arn
                );

                let page = listed(&emulator.router, &case, family.list, &job_name).await;
                assert_eq!(
                    page.len(),
                    1,
                    "case {case:?}: expected the accepted submission to be listed once, \
                     got {page:?}"
                );
            } else {
                let code = error_code(&answer);
                assert_eq!(
                    (status, code.as_str()),
                    (400, INVALID_REQUEST),
                    "case {case:?}: a role IAM answered this service cannot assume must be \
                     refused, got {status} {code:?}: {answer}"
                );
                assert_eq!(
                    answer["Message"].as_str(),
                    Some(NOT_TRUSTED),
                    "case {case:?}: expected the trust-resolution message, got {answer}"
                );
                assert!(
                    answer.get("JobId").is_none(),
                    "case {case:?}: a refused submission mints no JobId, got {answer}"
                );

                // The refusal persisted NOTHING, so the family's list reports no job for
                // it: the result set is the set of submissions the service accepted.
                let page = listed(&emulator.router, &case, family.list, &job_name).await;
                assert!(
                    page.is_empty(),
                    "case {case:?}: a refused submission must leave no listable job, \
                     got {page:?}"
                );
            }
        }
    }
}

/// RULE (the complement): the refusal is grounded in what IAM ANSWERED, so an emulator
/// with no IAM mounted to ask refuses nothing — the same submission that is refused beside
/// IAM is accepted when there is no IAM to answer for the role.
#[tokio::test]
async fn a_start_refuses_nothing_when_there_is_no_iam_to_ask() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
            engine,
        ),
    );

    // The value the suite above sees refused: an ARN of this account naming a role IAM
    // holds nothing under. With no IAM to answer, nothing is known about it.
    let role_arn = format!(
        "arn:aws:iam::{}:role/cm-data-access-absent",
        ::engine::emulator_account_id()
    );
    for family in families() {
        let case = format!("{} with no IAM mounted", family.name);
        let (status, answer) = call(
            &router,
            family.start,
            &json!({
                "JobName": "probe-no-iam",
                "InputDataConfig": { "S3Bucket": "in-bucket", "S3Key": "docs/" },
                "OutputDataConfig": { "S3Bucket": "out-bucket", "S3Key": "out/" },
                "DataAccessRoleArn": role_arn,
                "LanguageCode": "en",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refusal has to be grounded in what IAM answered, got \
             {status} {answer}"
        );
        assert!(
            answer["JobId"].as_str().is_some(),
            "case {case:?}: expected a JobId, got {answer}"
        );
    }
}
