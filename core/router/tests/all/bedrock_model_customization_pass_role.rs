// SPDX-License-Identifier: CC0-1.0

//! Amazon Bedrock settles the `iam:PassRole` step of `CreateModelCustomizationJob` against
//! this account's IAM.
//!
//! RULE: a model customization makes Amazon Bedrock TAKE ON the submitted `roleArn`, so the
//! create is authorized before anything it names is resolved and is admitted only where the
//! value names a role of the CALLER's OWN account, this account's IAM HOLDS that role, and
//! the trust policy the role carries grants `bedrock.amazonaws.com` (the model's own
//! `awsService.cloudTrailEventSource`) the `sts:AssumeRole` THIS pass would make — a
//! statement's `Condition` being part of its grant, decided against the keys the pass
//! carries (`aws:SourceAccount`, the account whose request the service would act for) and
//! undecided, hence no grant, for any other key: the job the role would be assumed for does
//! not exist while its own creation is still being authorized, and the corpus records this
//! operation refusing a pass whose grant is narrowed to that job's ARN. Anything else is the
//! modelled `AccessDeniedException` (HTTP 403) carrying the one sentence that denial is
//! stated with, and such a refusal WRITES NOTHING: no job, no reserved custom-model name.
//!
//! The role is resolved through the in-process callback, so a single-service router has no
//! IAM to ask: that is why this suite lives here rather than in the service's own crate,
//! whose suites pin the half of the rule that needs no IAM.
//!
//! The rows span what the rule generalises over: the trust policy's shape (`Action` and
//! `Principal.Service` as one string or as a list, the wildcards IAM admits in the action's
//! place, an explicit `Deny` beside an `Allow`, a grant to an account principal rather than
//! to a service one, a policy naming only another service), which condition the grant is
//! narrowed by (none, a key the
//! pass carries and holds, the same key with another account's value, a key the pass carries
//! no value for), where the role lives (the default path and a path), which account the ARN
//! names, whether the role was ever created, and — because the decision is taken over the
//! request as submitted — every form `baseModelIdentifier` takes that IDENTIFIES a
//! customizable base model variant (a `:foundation-model/` ARN, a `:custom-model/` ARN, a
//! bare id carrying the version qualifier such a variant is published under) whether or not
//! it resolves to anything this service publishes. A value that identifies no such variant —
//! a bare `vendor.model` id with no qualifier — is read AHEAD of the pass, so those rows are
//! answered by that reading instead; the corpus records both halves of that ordering.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder an awsQuery form body needs is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns: a form component is escaped
// exactly as a URI path segment is, so the codec is shared rather than written again.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here so this suite
/// pins the wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The sentence Amazon Bedrock states for a value that identifies no customizable base
/// model variant, spelled here so this suite pins the wording a client sees.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";

/// `ModelCustomizationJobStatus` — the status a job that has just been created carries
/// (`smithy_query(query="shape",shape="ModelCustomizationJobStatus")`).
const IN_PROGRESS: &str = "InProgress";

/// The service principal a customization role's trust policy has to grant the assume to,
/// spelled as a JSON string so a row can name it beside others in a list.
const SERVICE_PRINCIPAL: &str = "\"bedrock.amazonaws.com\"";

/// The Bedrock control plane and IAM over ONE shared engine, as `router/src/lib.rs` mounts
/// them, so the control plane's in-process callback reaches this very IAM.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One awsQuery call to IAM: `POST /` with the parameters in the form body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One restJson1 call to the Bedrock control plane, answered with the status, the wire
/// error code and the JSON body.
async fn bedrock(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(bare_code);
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let error = header_error.unwrap_or_else(|| {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(bare_code)
            .unwrap_or_default()
    });
    (status, error, parsed)
}

/// The bare error code out of an `__type` / `x-amzn-errortype` value, which may carry a
/// namespace or a URL beside it.
fn bare_code(raw: &str) -> String {
    let without_url = raw.split(':').next().unwrap_or(raw);
    without_url
        .rsplit('#')
        .next()
        .unwrap_or(without_url)
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of a role of this account, under the default path.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A trust policy with one statement, built from the pieces that vary between rows: the
/// effect, the action(s), the service principal(s) and the `Condition` the grant is narrowed
/// by (`""` for a statement that carries none).
fn trust_policy(effect: &str, action: &str, service: &str, condition: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}{condition}}}]}}"
    )
}

/// A `Condition` element as a statement member, ready to append to one.
fn condition(body: &str) -> String {
    format!(",\"Condition\":{body}")
}

/// One IAM role the rows name, as `CreateRole` parameters.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "customization-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "customization-role-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"lambda.amazonaws.com\",\"bedrock.amazonaws.com\"]",
                "",
            ),
        },
        RoleFixture {
            role_name: "customization-role-wildcard",
            path: None,
            policy: trust_policy("Allow", "\"sts:*\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "customization-role-path",
            path: Some("/service-role/"),
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            // The pass carries this account as `aws:SourceAccount`, so the clause is
            // decided and holds: the grant covers the pass.
            role_name: "customization-role-source-account",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                SERVICE_PRINCIPAL,
                &condition(&format!(
                    "{{\"StringEquals\":{{\"aws:SourceAccount\":\"{ACCOUNT}\"}}}}"
                )),
            ),
        },
        RoleFixture {
            // The same clause over ANOTHER account: decided, and it does not hold.
            role_name: "customization-role-other-source-account",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                SERVICE_PRINCIPAL,
                &condition(
                    "{\"StringEquals\":{\"aws:SourceAccount\":\"123456789012\"}}",
                ),
            ),
        },
        RoleFixture {
            // A grant narrowed by BOTH confused-deputy keys. The account one holds — the
            // pass carries that account — while the source ARN is the ARN of the job the
            // role would be assumed for, and that job does not exist while its own creation
            // is still being authorized: the pass carries no value for the key, so the
            // grant cannot be shown to cover this pass. RECORDED: the corpus records this
            // operation refusing a pass of exactly this shape.
            role_name: "customization-role-source-arn",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                SERVICE_PRINCIPAL,
                &condition(&format!(
                    "{{\"StringEquals\":{{\"aws:SourceAccount\":\"{ACCOUNT}\"}},\
                     \"ArnEquals\":{{\"aws:SourceArn\":\
                     \"arn:aws:bedrock:{REGION}:{ACCOUNT}:model-customization-job/*\"}}}}"
                )),
            ),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"lambda.amazonaws.com\"",
                "",
            ),
        },
        RoleFixture {
            role_name: "other-action-role",
            path: None,
            policy: trust_policy("Allow", "\"s3:GetObject\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            // IAM's explicit deny wins over an `Allow` for the same principal.
            role_name: "allow-then-deny-role",
            path: None,
            policy: format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
                 \"Action\":\"sts:AssumeRole\"}},\
                 {{\"Effect\":\"Deny\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
                 \"Action\":\"sts:AssumeRole\"}}]}}"
            ),
        },
        RoleFixture {
            // A document that IS a policy and grants the assume to an ACCOUNT principal
            // rather than to a service one, so it grants nothing to Amazon Bedrock.
            role_name: "account-principal-role",
            path: None,
            policy: format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
                 \"Action\":\"sts:AssumeRole\"}}]}}"
            ),
        },
    ]
}

/// One row: the role a create passes, and whether the pass is admitted.
struct Row {
    case: &'static str,
    /// The stem this row's `jobName` and `customModelName` are derived from, so each row
    /// owns its own names. `JobName` is `^[a-zA-Z0-9](-*[a-zA-Z0-9\+\-\.])*$` and
    /// `CustomModelName` is `^([0-9a-zA-Z][_-]?){1,63}$`, so one stem of letters and dashes
    /// suits both.
    stem: &'static str,
    role_arn: String,
    admitted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role this account holds that grants the assume",
            stem: "trusted",
            role_arn: role_arn("customization-role"),
            admitted: true,
        },
        Row {
            case: "a trust policy whose Action and Service are lists",
            stem: "trusted-lists",
            role_arn: role_arn("customization-role-lists"),
            admitted: true,
        },
        Row {
            case: "a trust policy granting the whole sts namespace",
            stem: "trusted-wildcard",
            role_arn: role_arn("customization-role-wildcard"),
            admitted: true,
        },
        Row {
            case: "a role under a path",
            stem: "trusted-path",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/service-role/customization-role-path"),
            admitted: true,
        },
        Row {
            case: "a grant conditioned on the account the pass carries",
            stem: "trusted-source-account",
            role_arn: role_arn("customization-role-source-account"),
            admitted: true,
        },
        Row {
            case: "a grant conditioned on another account",
            stem: "other-source-account",
            role_arn: role_arn("customization-role-other-source-account"),
            admitted: false,
        },
        Row {
            case: "a grant narrowed to the ARN of the job this call would create",
            stem: "source-arn",
            role_arn: role_arn("customization-role-source-arn"),
            admitted: false,
        },
        Row {
            case: "a role this account was never given",
            stem: "absent-role",
            role_arn: role_arn("never-created-role"),
            admitted: false,
        },
        Row {
            case: "a role that trusts another service only",
            stem: "lambda-role",
            role_arn: role_arn("lambda-only-role"),
            admitted: false,
        },
        Row {
            case: "a role whose statement names another action",
            stem: "other-action",
            role_arn: role_arn("other-action-role"),
            admitted: false,
        },
        Row {
            case: "a role whose statement denies the assume",
            stem: "denied-role",
            role_arn: role_arn("denied-role"),
            admitted: false,
        },
        Row {
            case: "a role that both grants and explicitly denies the assume",
            stem: "allow-then-deny",
            role_arn: role_arn("allow-then-deny-role"),
            admitted: false,
        },
        Row {
            case: "a role whose trust policy grants an account principal rather than a service one",
            stem: "account-principal",
            role_arn: role_arn("account-principal-role"),
            admitted: false,
        },
        Row {
            // `iam:PassRole` is granted by the account that HOLDS the role, so another
            // account's role is one no principal of this account can pass.
            case: "a role of another account",
            stem: "foreign-role",
            role_arn: "arn:aws:iam::123456789012:role/customization-role".to_string(),
            admitted: false,
        },
        Row {
            // The member's published pattern admits an EMPTY account field, which names no
            // OTHER account: this one's role, and this one holds no role under that name.
            case: "a role ARN carrying no account at all",
            stem: "no-account",
            role_arn: "arn:aws:iam:::role/never-created-role".to_string(),
            admitted: false,
        },
    ]
}

/// Create every role the rows name.
async fn create_roles(router: &MultiServiceRouter) {
    for fixture in role_fixtures() {
        let mut parameters = vec![
            ("RoleName", fixture.role_name),
            ("AssumeRolePolicyDocument", fixture.policy.as_str()),
        ];
        if let Some(path) = fixture.path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, "CreateRole", &parameters).await;
        assert_eq!(
            status, 200,
            "setup: CreateRole {} must succeed, got {status} {body}",
            fixture.role_name
        );
    }
}

/// A base model the catalog this emulator serves publishes as supporting `FINE_TUNING`,
/// read through `ListFoundationModels` exactly as a caller would — the training-qualified
/// id Amazon Bedrock publishes a customizable model under.
async fn fine_tuning_base_model(router: &MultiServiceRouter) -> String {
    let (status, _, listed) = bedrock(
        router,
        "GET",
        "/foundation-models?byCustomizationType=FINE_TUNING",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ListFoundationModels must answer 200, got {status} ({listed})"
    );
    listed["modelSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|model| {
            model["modelLifecycle"]["status"].as_str() == Some("ACTIVE")
                && model["customizationsSupported"]
                    .as_array()
                    .is_some_and(|supported| {
                        supported.iter().any(|value| value == "FINE_TUNING")
                    })
        })
        .map(|model| text(model, "modelId"))
        .expect("the catalog must publish an ACTIVE base model supporting FINE_TUNING")
}

/// Every `modelId` the catalog this emulator serves publishes, read through
/// `ListFoundationModels` exactly as a caller would.
async fn published_model_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, _, listed) = bedrock(router, "GET", "/foundation-models", None).await;
    assert_eq!(
        status, 200,
        "setup: ListFoundationModels must answer 200, got {status} ({listed})"
    );
    listed["modelSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|model| text(model, "modelId"))
        .collect()
}

/// Base model ids AWS publishes as past their END OF LIFE ("Model lifecycle" in the Amazon
/// Bedrock User Guide), so a service that offers only its live models publishes no catalog
/// row for any of them — which the control plane's own
/// `tests/all/get_foundation_model.rs` pins, and which the rows below re-read off
/// `ListFoundationModels` rather than assume. TWO of them, so the rule is not asserted on
/// the single id a check happens to read.
const END_OF_LIFE_MODEL_IDS: &[&str] = &["amazon.titan-text-express-v1", "anthropic.claude-v2"];

/// The modelled client errors of this operation that report an identifier the service does
/// not offer a base model under, with the `@httpError` each carries
/// (`smithy_query(query="operation",operation="CreateModelCustomizationJob")`): the request
/// named something, so the answer is about the thing it named — not the authorization, not a
/// conflict, not a quota and not a throttle.
const UNOFFERED_MODEL_ERRORS: &[(u16, &str)] = &[
    (400, "ValidationException"),
    (404, "ResourceNotFoundException"),
];

/// A `CreateModelCustomizationJob` body carrying the six `@required` members.
fn create_body(stem: &str, base_model: &str, role_arn: &str) -> Value {
    json!({
        "jobName": format!("job-{stem}"),
        "customModelName": format!("model-{stem}"),
        "baseModelIdentifier": base_model,
        "roleArn": role_arn,
        "trainingDataConfig": {"s3Uri": "s3://customization-data/train.jsonl"},
        "outputDataConfig": {"s3Uri": "s3://customization-output/"},
    })
}

/// How many rows a listing reports under `member`, with an omitted list read as none.
fn listed_count(body: &Value, member: &str) -> usize {
    body[member].as_array().map(Vec::len).unwrap_or_default()
}

#[tokio::test]
async fn a_customization_job_is_minted_only_for_a_role_iam_lets_bedrock_assume() {
    let router = emulator();
    create_roles(&router).await;
    let base_model = fine_tuning_base_model(&router).await;

    for row in rows() {
        let (status, error, body) = bedrock(
            &router,
            "POST",
            "/model-customization-jobs",
            Some(create_body(row.stem, &base_model, &row.role_arn)),
        )
        .await;

        if row.admitted {
            assert_eq!(
                (status, text(&body, "jobArn").is_empty()),
                (201, false),
                "case {:?} ({}): an admitted pass must mint a job, got {status} {error} ({body})",
                row.case,
                row.role_arn
            );
            // Read the job back through the operation a caller polls it with: the create
            // wrote the record every later request sees.
            let job_arn = text(&body, "jobArn");
            let label = job_arn.replace('%', "%25").replace(':', "%3A").replace('/', "%2F");
            let (status, error, read) = bedrock(
                &router,
                "GET",
                &format!("/model-customization-jobs/{label}"),
                None,
            )
            .await;
            assert_eq!(
                (
                    status,
                    text(&read, "jobArn").as_str(),
                    text(&read, "roleArn").as_str(),
                    text(&read, "status").as_str(),
                ),
                (200, job_arn.as_str(), row.role_arn.as_str(), IN_PROGRESS),
                "case {:?}: the job must read back as created under the role it was passed, \
                 got {status} {error} ({read})",
                row.case
            );
        } else {
            assert_eq!(
                (status, error.as_str(), text(&body, "message").as_str()),
                (403, "AccessDeniedException", PASS_ROLE_REFUSAL),
                "case {:?} ({}): a pass IAM does not authorize must be refused 403 \
                 AccessDeniedException, got {status} {error} ({body})",
                row.case,
                row.role_arn
            );
            assert!(
                body.get("jobArn").is_none(),
                "case {:?}: a refused create mints nothing, got {body}",
                row.case
            );
        }

        // What the refusal left behind, read through the listings a client checks: a
        // refused create leaves NO job and NO reserved custom-model name, and an admitted
        // one leaves exactly one of each.
        let expected = usize::from(row.admitted);
        let (status, error, jobs) = bedrock(
            &router,
            "GET",
            &format!("/model-customization-jobs?nameContains=job-{}", row.stem),
            None,
        )
        .await;
        assert_eq!(
            (status, listed_count(&jobs, "modelCustomizationJobSummaries")),
            (200, expected),
            "case {:?}: expected {expected} job summary(ies) under that name, got {status} \
             {error} ({jobs})",
            row.case
        );
        let (status, error, models) = bedrock(
            &router,
            "GET",
            &format!("/custom-models?nameContains=model-{}", row.stem),
            None,
        )
        .await;
        assert_eq!(
            (status, listed_count(&models, "modelSummaries")),
            (200, 0),
            "case {:?}: no custom model exists under that name yet — a refused create \
             reserved nothing at all, and an admitted one brings its model into being only \
             when the job it started completes — got {status} {error} ({models})",
            row.case
        );
    }
}

/// What a create answers once the pass-role decision has ADMITTED the pass: the row's own
/// resolution, which is what the ordering rule is about.
enum AdmittedAnswer {
    /// The identifier resolves to a base model the catalog publishes, so the job is minted.
    MintsJob,
    /// The identifier names no base model at all, which this service settles as the modelled
    /// `ResourceNotFoundException` (`@httpError(404)`).
    ReportsItAbsent,
    /// The identifier names a model AWS publishes as past its end of life, so the service
    /// offers no row for it. WHICH of this operation's modelled errors for an identifier it
    /// does not offer a model under reports a model that is known but no longer offered is
    /// the subject of its own open report, so this row holds the answer to
    /// [`UNOFFERED_MODEL_ERRORS`] and pins what the ordering rule settles: the answer is the
    /// identifier's own and not the authorization's.
    ReportsAClientError,
    /// The value identifies no customizable base model variant at all — a bare id carrying
    /// no version qualifier — which is read AHEAD of the pass, so this row is answered by
    /// that reading under EVERY role: 400 `ValidationException` carrying the service's fixed
    /// invalid-model-identifier sentence.
    RefusedOnItsForm,
}

/// The roles a pass is refused on, spanning both halves of a pass IAM withholds: a role of
/// the caller's OWN account that this account's IAM holds and that trusts this service, whose
/// grant is narrowed to the ARN of the very job the call would create — a key the pass
/// carries no value for, which is the shape the corpus records a refusal for — and a role
/// that trusts another service only.
const REFUSING_ROLES: &[(&str, &str)] = &[
    (
        "a grant narrowed to the ARN of the job this call would create",
        "customization-role-source-arn",
    ),
    ("a role that trusts another service only", "lambda-only-role"),
];

/// RULE (the ordering half): the pass is authorized BEFORE anything the request names is
/// RESOLVED, so the same body draws the 403 wherever the grant does not cover the pass —
/// whatever `baseModelIdentifier` would have resolved to, in every form that IDENTIFIES a
/// customizable base model variant (a `:foundation-model/` ARN, a `:custom-model/` ARN, a
/// bare id carrying that variant's version qualifier) — and draws that identifier's OWN
/// answer wherever the grant does cover the pass, so an admitting decision does not swallow
/// what follows it either. Ahead of both stands the FORM of the identifier: a bare id
/// carrying no qualifier identifies no variant to customize and is refused under every role.
#[tokio::test]
async fn the_pass_is_authorized_before_the_request_is_resolved() {
    let router = emulator();
    create_roles(&router).await;
    let published = fine_tuning_base_model(&router).await;
    let catalogued = published_model_ids(&router).await;
    for id in END_OF_LIFE_MODEL_IDS {
        assert!(
            !catalogued.iter().any(|model_id| model_id == id),
            "setup: {id:?} is past its end of life, so the catalog must publish no row for \
             it, got {catalogued:?}"
        );
    }

    // The rows differ along the FORM the identifier takes and along whether it resolves at
    // all, which is what the rule generalises over.
    let rows: Vec<(&str, String, AdmittedAnswer)> = vec![
        (
            "the published base model, by its qualified bare id",
            published.clone(),
            AdmittedAnswer::MintsJob,
        ),
        (
            "the published base model, by its foundation-model ARN",
            format!("arn:aws:bedrock:{REGION}::foundation-model/{published}"),
            AdmittedAnswer::MintsJob,
        ),
        (
            "a model past its end of life, by its bare id",
            END_OF_LIFE_MODEL_IDS[0].to_string(),
            AdmittedAnswer::RefusedOnItsForm,
        ),
        (
            "a model past its end of life, by its foundation-model ARN",
            format!(
                "arn:aws:bedrock:{REGION}::foundation-model/{}",
                END_OF_LIFE_MODEL_IDS[0]
            ),
            AdmittedAnswer::ReportsAClientError,
        ),
        (
            "another model past its end of life, by its foundation-model ARN",
            format!(
                "arn:aws:bedrock:{REGION}::foundation-model/{}",
                END_OF_LIFE_MODEL_IDS[1]
            ),
            AdmittedAnswer::ReportsAClientError,
        ),
        (
            "an id that names no model at all",
            "acme.not-a-model-v1".to_string(),
            AdmittedAnswer::RefusedOnItsForm,
        ),
        (
            "a custom-model ARN this account does not hold",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{published}/abcdef012345"),
            AdmittedAnswer::ReportsItAbsent,
        ),
    ];

    for (row_index, (case, base_model, admitted)) in rows.iter().enumerate() {
        for (role_index, (role_case, role)) in REFUSING_ROLES.iter().enumerate() {
            let stem = format!("spelling-{row_index}-{role_index}");
            let (status, error, refused) = bedrock(
                &router,
                "POST",
                "/model-customization-jobs",
                Some(create_body(&stem, base_model, &role_arn(role))),
            )
            .await;
            let expected = match admitted {
                // The identifier's own form is read first, so this row is answered by that
                // reading whatever the role is.
                AdmittedAnswer::RefusedOnItsForm => {
                    (400, "ValidationException", INVALID_MODEL_IDENTIFIER)
                }
                _ => (403, "AccessDeniedException", PASS_ROLE_REFUSAL),
            };
            assert_eq!(
                (status, error.as_str(), text(&refused, "message").as_str()),
                expected,
                "case {case:?} ({base_model}) with {role_case:?}: the pass decision comes \
                 first for every identifier it resolves, got {status} {error} ({refused})"
            );

            // The refusal wrote nothing: no job under the name it carried, and no custom
            // model reserved under the one it asked for.
            let (status, error, jobs) = bedrock(
                &router,
                "GET",
                &format!("/model-customization-jobs?nameContains=job-{stem}"),
                None,
            )
            .await;
            assert_eq!(
                (status, listed_count(&jobs, "modelCustomizationJobSummaries")),
                (200, 0),
                "case {case:?} with {role_case:?}: a refused pass leaves no job to list, got \
                 {status} {error} ({jobs})"
            );
            let (status, error, models) = bedrock(
                &router,
                "GET",
                &format!("/custom-models?nameContains=model-{stem}"),
                None,
            )
            .await;
            assert_eq!(
                (status, listed_count(&models, "modelSummaries")),
                (200, 0),
                "case {case:?} with {role_case:?}: a refused pass reserves no custom model, \
                 got {status} {error} ({models})"
            );
        }

        // The same identifier, passed a role whose grant DOES cover the pass: now the
        // request's own resolution is what answers.
        let (status, error, resolved) = bedrock(
            &router,
            "POST",
            "/model-customization-jobs",
            Some(create_body(
                &format!("spelling-admitted-{row_index}"),
                base_model,
                &role_arn("customization-role"),
            )),
        )
        .await;
        match admitted {
            AdmittedAnswer::MintsJob => assert_eq!(
                (status, text(&resolved, "jobArn").is_empty()),
                (201, false),
                "case {case:?} ({base_model}): an admitted pass over an identifier the \
                 catalog publishes mints a job, got {status} {error} ({resolved})"
            ),
            AdmittedAnswer::ReportsItAbsent => assert_eq!(
                (status, error.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} ({base_model}): with a passable role the identifier's own \
                 resolution answers, got {status} {error} ({resolved})"
            ),
            AdmittedAnswer::RefusedOnItsForm => assert_eq!(
                (status, error.as_str(), text(&resolved, "message").as_str()),
                (400, "ValidationException", INVALID_MODEL_IDENTIFIER),
                "case {case:?} ({base_model}): a value identifying no customizable base \
                 model variant is refused on its form whatever the role, got {status} \
                 {error} ({resolved})"
            ),
            AdmittedAnswer::ReportsAClientError => assert!(
                UNOFFERED_MODEL_ERRORS
                    .iter()
                    .any(|(modelled, code)| (*modelled, *code) == (status, error.as_str())),
                "case {case:?} ({base_model}): with a passable role a model this service \
                 does not offer is reported by one of this operation's own modelled errors \
                 for the identifier it named ({UNOFFERED_MODEL_ERRORS:?}) rather than by the \
                 authorization, got {status} {error} ({resolved})"
            ),
        }
    }
}

