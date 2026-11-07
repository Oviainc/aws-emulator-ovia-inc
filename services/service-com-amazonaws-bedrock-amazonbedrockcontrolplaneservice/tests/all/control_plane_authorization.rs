// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's control plane, and WHICH ROLE a caller may hand it, driven
//! over the real wire path (a restJson1 request through the multi-service router into the
//! mounted service beside IAM and STS on one engine), so what is asserted is what a client
//! sees.
//!
//! Two rules, each asserted over a TABLE rather than over one request:
//!
//! 1. **Every operation of this service is authorized before it reads or writes anything.**
//!    AWS takes IAM's decision in front of the service, so a request is served only where
//!    the identity-based policies in force on the principal its credentials name allow
//!    `bedrock:<OperationName>` on the resource the request addressed; a request no
//!    statement allows, or one an explicit `Deny` covers, is refused with the modelled
//!    `AccessDeniedException` (`@httpError(403)`) and performs nothing. The rows span what
//!    the rule generalises over: the OPERATION (a create, a read addressed by a bare id, the
//!    same read addressed by an ARN, a listing, a delete addressed by an ARN in the path, a
//!    read addressed by an ARN in the BODY, an account-level put and its read, and the two
//!    job creates), the DOCUMENT in force (none at all, another action of this service only,
//!    another service's actions only, an allow of exactly the action, an allow of the whole
//!    service, an allow of everything beside an explicit deny of the action, and an explicit
//!    deny of everything — the shape a scoped-down role carries), and the CREDENTIAL (an
//!    assumed-role session, the account's own published key). The refusal's sentence is
//!    asserted whole, with every part — principal, action, resource, reason — taken from
//!    the request and the stored policy, and the resource it names being the one the
//!    request ADDRESSED: the ARN where the request carries one, that same ARN where the
//!    request carries the resource's own ARN id in its URI, the collection of the family
//!    whose URI space it was sent to where it names no single member of it, and no resource
//!    clause at all where the operation addresses no resource of this service.
//! 2. **A job create decides the `iam:PassRole` of the role it hands Amazon Bedrock.** The
//!    job creates pass a role the service is to run the job under, so the create is admitted
//!    only where this account's IAM holds that role and its trust policy grants
//!    `bedrock.amazonaws.com` the `sts:AssumeRole` THIS pass would make — a statement's
//!    `Condition` being part of its grant, decided against the keys the pass carries. Any
//!    other answer is the modelled `AccessDeniedException` carrying the one sentence that
//!    denial is stated with, and such a refusal writes no job: the listing filtered by the
//!    `jobName` it carried stays empty. The rows span the trust policy's shape, the
//!    condition it is narrowed by, which account the role ARN names, and whether IAM holds
//!    the role at all — for BOTH job creates this change adds the decision to.
//!
//! Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="CreateGuardrail,DeleteResourcePolicy,ListTagsForResource,PutAccountDataRetention,CreateModelInvocationJob,CreateEvaluationJob")`)
//! or from the published foundation-model catalog this service serves.
//!
//! The emulator, the signed call, the seeded session and the policy documents are the
//! harness `crate::custom_model_authorization` owns, reused rather than copied: one rule,
//! one dispatch path.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use crate::custom_model_authorization::{
    allow, allow_all_but, emulator, given_role_policy, given_session, session_arn, Emulator,
    Verdict, ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::support::{foundation_model_arn, label, Reply, ACCOUNT, REGION};

/// The published foundation model catalog this service serves (`state::FOUNDATION_MODELS`),
/// so a `modelId` these rows carry follows the catalog rather than a pinned spelling.
const CATALOG: &[service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FoundationModel] =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS;

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here so this suite
/// pins the wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The service principal a job role's trust policy has to grant the assume to: this
/// service's own `awsService$cloudTrailEventSource`.
pub(crate) const SERVICE_PRINCIPAL: &str = "bedrock.amazonaws.com";

/// The role a job create hands Amazon Bedrock, as IAM holds it.
pub(crate) const JOB_ROLE: &str = "BedrockJobExecution";

/// The resource a refusal of a request that addresses no resource of this service is
/// judged against, as IAM writes it for such a request — the sentence states no resource
/// clause for it.
const ANY_RESOURCE: &str = "*";

/// The collection of `resource_type` in this account and Region: the ARN IAM evaluates an
/// action against when the request names no single resource, written the way this service
/// writes the ARNs of the resources it holds.
fn collection(resource_type: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/*")
}

// ---------------------------------------------------------------------------------------
// The identity-based decision.
// ---------------------------------------------------------------------------------------

/// A policy allowing every action of this service on `resource` ALONE — the shape a
/// least-privilege policy scoped to one resource or to one family takes.
fn allow_on(resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "bedrock:*", "Resource": resource }]
    })
}

/// A policy denying every action on every resource — the shape a scoped-down role carries,
/// where an explicit `Deny` beats any `Allow` the principal also holds.
fn deny_everything() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "*", "Resource": "*" },
            { "Effect": "Deny", "Action": "*", "Resource": "*" },
        ]
    })
}

/// One control-plane request: the operation IAM is asked about, the call that makes it, and
/// the resource the refusal must name.
struct Call {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
}

/// The `jobName` the two job-create rows carry, unique per call so a replay of one row is
/// never answered out of an idempotency record.
fn job_name(prefix: &str, suffix: &str) -> String {
    format!("{prefix}-{suffix}")
}

/// A `CreateModelInvocationJob` body carrying only the members the model marks required.
///
/// `modelId` admits a bare model id and a model ARN
/// (`smithy_query(query="operation",operation="CreateModelInvocationJob")`), and the ARN
/// spelling is what proves the action is judged against the JOB collection rather than
/// against the model the job runs.
pub(crate) fn invocation_job_body(name: &str, role_arn: &str) -> Value {
    invocation_job_body_for(name, role_arn, CATALOG[0].model_id)
}

/// The same body, naming the model by `model_id` — a bare id or a model ARN.
fn invocation_job_body_for(name: &str, role_arn: &str, model_id: &str) -> Value {
    json!({
        "jobName": name,
        "roleArn": role_arn,
        "modelId": model_id,
        "inputDataConfig": { "s3InputDataConfig": { "s3Uri": "s3://batch-in/prompts.jsonl" } },
        "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "s3://batch-out/" } },
    })
}

/// A `CreateEvaluationJob` body carrying only the members the model marks required.
pub(crate) fn evaluation_job_body(name: &str, role_arn: &str) -> Value {
    json!({
        "jobName": name,
        "roleArn": role_arn,
        "outputDataConfig": { "s3Uri": "s3://evaluation-out/" },
        "evaluationConfig": {
            "automated": {
                "datasetMetricConfigs": [{
                    "taskType": "Summarization",
                    "dataset": { "name": "Builtin.Gigaword" },
                    "metricNames": ["Builtin.Accuracy"],
                }],
            },
        },
        "inferenceConfig": {
            "models": [{
                "bedrockModel": {
                    "modelIdentifier": CATALOG[0].model_id,
                    "inferenceParams": "{\"temperature\":0.2}",
                },
            }],
        },
    })
}

/// Every class of request a credential can reach this service with, one row per thing the
/// rule generalises over, built for the `case` suffix so two rows never collide on a name.
fn calls(guardrail_id: &str, guardrail_arn: &str, suffix: &str) -> Vec<Call> {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");
    vec![
        Call {
            case: "a create",
            operation: "CreateGuardrail",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "name": format!("authorization-{suffix}"),
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
            resource: collection("guardrail"),
        },
        Call {
            case: "a read addressed by a bare id",
            operation: "GetGuardrail",
            method: "GET",
            path: format!("/guardrails/{}", label(guardrail_id)),
            body: None,
            // A bare `guardrailIdentifier` IS the id inside the guardrail's own ARN, so the
            // request addresses that one guardrail just as spelling its ARN does.
            resource: guardrail_arn.to_string(),
        },
        Call {
            case: "a read addressed by the guardrail's ARN",
            operation: "GetGuardrail",
            method: "GET",
            path: format!("/guardrails/{}", label(guardrail_arn)),
            body: None,
            // An ARN names the resource itself, so that is what the action is judged on.
            resource: guardrail_arn.to_string(),
        },
        Call {
            case: "a listing",
            operation: "ListGuardrails",
            method: "GET",
            path: "/guardrails".to_string(),
            body: None,
            resource: collection("guardrail"),
        },
        Call {
            case: "a delete addressed by an ARN in the path",
            operation: "DeleteResourcePolicy",
            method: "DELETE",
            path: format!("/resource-policy/{}", label(guardrail_arn)),
            body: None,
            resource: guardrail_arn.to_string(),
        },
        Call {
            case: "a read addressed by an ARN in the body",
            operation: "ListTagsForResource",
            method: "POST",
            path: "/listTagsForResource".to_string(),
            body: Some(json!({ "resourceARN": guardrail_arn })),
            resource: guardrail_arn.to_string(),
        },
        Call {
            case: "an account-level put",
            operation: "PutAccountDataRetention",
            method: "PUT",
            path: "/data-retention".to_string(),
            body: Some(json!({ "mode": "ENABLED" })),
            resource: ANY_RESOURCE.to_string(),
        },
        Call {
            case: "an account-level read",
            operation: "GetAccountDataRetention",
            method: "GET",
            path: "/data-retention".to_string(),
            body: None,
            resource: ANY_RESOURCE.to_string(),
        },
        Call {
            case: "a batch inference job create",
            operation: "CreateModelInvocationJob",
            method: "POST",
            path: "/model-invocation-job".to_string(),
            body: Some(invocation_job_body(
                &job_name("batch", suffix),
                &role_arn,
            )),
            resource: collection("model-invocation-job"),
        },
        Call {
            // The request CARRIES a foundation-model ARN, which is a resource of another
            // family: the action still addresses the job collection, so that is what the
            // decision and the refusal name.
            case: "a batch inference job create naming its model by ARN",
            operation: "CreateModelInvocationJob",
            method: "POST",
            path: "/model-invocation-job".to_string(),
            body: Some(invocation_job_body_for(
                &job_name("batcharn", suffix),
                &role_arn,
                &foundation_model_arn(REGION, CATALOG[0].model_id),
            )),
            resource: collection("model-invocation-job"),
        },
        Call {
            case: "an evaluation job create",
            operation: "CreateEvaluationJob",
            method: "POST",
            path: "/evaluation-jobs".to_string(),
            body: Some(evaluation_job_body(
                &job_name("evaluation", suffix),
                &role_arn,
            )),
            resource: collection("evaluation-job"),
        },
    ]
}

/// One guardrail and one job role, created with the account's own credential — the
/// precondition the ARN-addressed rows and the admitted job creates need.
async fn fixture(emu: &Emulator) -> (String, String) {
    let reply = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            "/guardrails",
            Some(json!({
                "name": "authorization-fixture",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
        )
        .await;
    assert_eq!(
        reply.status, 202,
        "the account's own credential must be served, got {} {:?}",
        reply.status, reply.body
    );
    (reply.text("guardrailId"), reply.text("guardrailArn"))
}

/// Record a role IAM holds under `name`, carrying `trust` as its trust policy — what
/// `iam:CreateRole` records.
pub(crate) fn given_role(engine: &Arc<engine::InMemoryEngine>, name: &str, trust: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.insert(
        iam::name_key(name),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: name.to_string(),
            role_id: "AROAEXAMPLEBEDROCKJOBS".to_string(),
            arn: format!("arn:aws:iam::{ACCOUNT}:role/{name}"),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: trust.map(ToString::to_string),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// A trust policy granting `SERVICE_PRINCIPAL` the assume, optionally narrowed by
/// `condition`.
pub(crate) fn trust(condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": SERVICE_PRINCIPAL },
        "Action": "sts:AssumeRole",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// RULE 1: every operation of this service evaluates the caller's identity-based policies
/// against `bedrock:<OperationName>` on the resource the request addressed, refusing with
/// 403 `AccessDeniedException` — and IAM's own sentence — what no statement allows or a
/// `Deny` covers, while the account's own credential is served.
#[tokio::test]
async fn every_control_plane_operation_is_authorized_against_the_callers_policies() {
    let emu = emulator();
    let (guardrail_id, guardrail_arn) = fixture(&emu).await;
    given_session(&emu.engine);
    // The role the job creates pass, held by this account and trusted to this service, so
    // these rows turn on the IDENTITY making the call rather than on the role it passes.
    given_role(&emu.engine, JOB_ROLE, Some(&trust(None)));

    // (case, the document in force on the role, the verdict every call draws under it)
    let documents: Vec<(&str, Option<Value>, Verdict)> = vec![
        ("no policy at all", None, Verdict::DefaultDeny),
        (
            "another action of this service only",
            Some(allow(json!("bedrock:ListFoundationModels"))),
            Verdict::DefaultDeny,
        ),
        (
            "another service's actions only",
            Some(allow(json!("s3:*"))),
            Verdict::DefaultDeny,
        ),
        (
            "an allow of this whole service",
            Some(allow(json!("bedrock:*"))),
            Verdict::Served,
        ),
        (
            "an explicit deny of everything beside an allow of everything",
            Some(deny_everything()),
            Verdict::ExplicitDeny,
        ),
    ];

    for (index, (document_case, document, verdict)) in documents.iter().enumerate() {
        given_role_policy(&emu.engine, document.as_ref());
        for call in calls(&guardrail_id, &guardrail_arn, &format!("doc{index}")) {
            let case = format!("{document_case:?} / {:?}", call.case);
            let reply = emu
                .call_as(SESSION_KEY_ID, call.method, &call.path, call.body.clone())
                .await;
            if *verdict == Verdict::Served {
                assert_ne!(
                    reply.error.as_deref(),
                    Some("AccessDeniedException"),
                    "case {case}: a matching Allow must not draw AccessDeniedException, got \
                     {} {:?}",
                    reply.status,
                    reply.body
                );
            } else {
                assert_refused(&case, &reply, call.operation, &call.resource, *verdict);
            }

            // The same call with the account's own published credential, which holds no IAM
            // policies: the contrast the rule rests on is the IDENTITY and nothing else.
            let served = emu
                .call_as(ACCOUNT_KEY_ID, call.method, &call.path, call.body.clone())
                .await;
            assert_ne!(
                served.error.as_deref(),
                Some("AccessDeniedException"),
                "case {case}: the account's own credential must not be refused, got {} {:?}",
                served.status,
                served.body
            );
        }
    }

    // The explicit-deny half scoped to ONE action, with the same calls under a document
    // that allows everything and denies just that one: the refusal must attribute itself to
    // that deny, and the allow of every other action must leave the rest served.
    for (index, call) in calls(&guardrail_id, &guardrail_arn, "scoped")
        .into_iter()
        .enumerate()
    {
        let action = format!("bedrock:{}", call.operation);
        given_role_policy(&emu.engine, Some(&allow_all_but(&action)));
        let case = format!("\"an explicit deny of {action}\" / {:?}", call.case);
        let reply = emu
            .call_as(SESSION_KEY_ID, call.method, &call.path, call.body.clone())
            .await;
        assert_refused(
            &case,
            &reply,
            call.operation,
            &call.resource,
            Verdict::ExplicitDeny,
        );

        // Every OTHER call of the table is allowed by the same document, so the deny is
        // scoped to its action rather than shutting the service.
        for other in calls(&guardrail_id, &guardrail_arn, &format!("scoped{index}")) {
            if other.operation == call.operation {
                continue;
            }
            let other_case = format!("{case} / also {:?}", other.case);
            let reply = emu
                .call_as(SESSION_KEY_ID, other.method, &other.path, other.body)
                .await;
            assert_ne!(
                reply.error.as_deref(),
                Some("AccessDeniedException"),
                "case {other_case}: an action the document allows must not be refused, got \
                 {} {:?}",
                reply.status,
                reply.body
            );
        }
    }
}

/// The refusal one row expects: 403, the modelled error code, and IAM's own sentence with
/// every part taken from the request and the stored policy — and no resource clause at all
/// for an action the request names no resource for.
fn assert_refused(case: &str, reply: &Reply, operation: &str, resource: &str, verdict: Verdict) {
    let action = format!("bedrock:{operation}");
    assert_eq!(
        reply.status, 403,
        "case {case}: expected the modelled 403, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {:?}",
        reply.error
    );
    let on_resource = if resource == ANY_RESOURCE {
        String::new()
    } else {
        format!(" on resource: {resource}")
    };
    let expected = format!(
        "User: {} is not authorized to perform: {action}{on_resource} {}",
        session_arn(),
        verdict.reason(&action)
    );
    assert_eq!(
        reply.get("message"),
        json!(expected),
        "case {case}: expected IAM's wording {expected:?}, got {:?}",
        reply.body
    );
}

/// RULE 1, the resource half: the resource an action is judged against is the one the
/// request ADDRESSED — the ARN it spells OF the family its URI space addresses, the ARN
/// built from that one resource's own id where the URI carries the id instead, or the
/// family's collection where it addresses no single member — and never an ARN of another
/// family the request happens to carry, so a policy scoped to what the request addressed
/// serves it and one scoped to another family does not. The id-addressed form has its own
/// table in `crate::id_addressed_resource_authorization`.
#[tokio::test]
async fn an_action_is_judged_against_the_resource_the_request_addressed() {
    let emu = emulator();
    let (_guardrail_id, guardrail_arn) = fixture(&emu).await;
    given_session(&emu.engine);
    given_role(&emu.engine, JOB_ROLE, Some(&trust(None)));
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");
    let model_arn = foundation_model_arn(REGION, CATALOG[0].model_id);

    // Each row is one request and the resource its action addresses, carried in the same
    // [`Call`] shape the table above uses.
    let rows: Vec<Call> = vec![
        Call {
            // The row the model ARN makes interesting: the request carries a
            // `foundation-model` ARN in `modelId`, and the action addresses the JOB.
            case: "a batch inference job create naming its model by ARN",
            operation: "CreateModelInvocationJob",
            method: "POST",
            path: "/model-invocation-job".to_string(),
            body: Some(invocation_job_body_for(
                "scopedbatcharn",
                &role_arn,
                &model_arn,
            )),
            resource: collection("model-invocation-job"),
        },
        Call {
            case: "a batch inference job create naming its model by id",
            operation: "CreateModelInvocationJob",
            method: "POST",
            path: "/model-invocation-job".to_string(),
            body: Some(invocation_job_body("scopedbatch", &role_arn)),
            resource: collection("model-invocation-job"),
        },
        Call {
            case: "an evaluation job create",
            operation: "CreateEvaluationJob",
            method: "POST",
            path: "/evaluation-jobs".to_string(),
            body: Some(evaluation_job_body("scoped-evaluation", &role_arn)),
            resource: collection("evaluation-job"),
        },
        Call {
            case: "a guardrail create",
            operation: "CreateGuardrail",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "name": "scoped-guardrail",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
            resource: collection("guardrail"),
        },
        Call {
            case: "a read addressed by the guardrail's own ARN",
            operation: "GetGuardrail",
            method: "GET",
            path: format!("/guardrails/{}", label(&guardrail_arn)),
            body: None,
            resource: guardrail_arn.clone(),
        },
        Call {
            case: "a read whose ARN names the resource of any family",
            operation: "ListTagsForResource",
            method: "POST",
            path: "/listTagsForResource".to_string(),
            body: Some(json!({ "resourceARN": guardrail_arn })),
            resource: guardrail_arn.clone(),
        },
        Call {
            // A free-text member a caller puts an ARN into addresses nothing: the model
            // declares `tags[].value` as neither an ARN member nor a top-level one, so the
            // action is still judged against the family this create addresses.
            case: "a create whose tag value carries another resource's ARN",
            operation: "CreateGuardrail",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "name": "scoped-tagged-guardrail",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
                "tags": [{ "key": "ref", "value": guardrail_arn }],
            })),
            resource: collection("guardrail"),
        },
        Call {
            // The same, where the space's target IS the ARN the request carries: the
            // `resourceARN` member decides it, not whichever leaf the codec reached first.
            case: "a tagging call whose tag value carries another resource's ARN",
            operation: "TagResource",
            method: "POST",
            path: "/tagResource".to_string(),
            body: Some(json!({
                "resourceARN": guardrail_arn,
                "tags": [{
                    "key": "ref",
                    "value": format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:imported-model/0123456789ab"),
                }],
            })),
            resource: guardrail_arn.clone(),
        },
    ];

    for call in rows {
        let Call {
            case,
            operation,
            method,
            path,
            body,
            resource: addressed,
        } = call;
        // Scoped to exactly what the request addressed: served.
        given_role_policy(&emu.engine, Some(&allow_on(&addressed)));
        let served = emu
            .call_as(SESSION_KEY_ID, method, &path, body.clone())
            .await;
        assert_ne!(
            served.error.as_deref(),
            Some("AccessDeniedException"),
            "case {case:?}: a policy scoped to {addressed} must serve the call, got {} {:?}",
            served.status,
            served.body
        );

        // Scoped to ANOTHER family of this service: the default deny, and the sentence
        // names the resource the request addressed rather than any ARN it carried.
        given_role_policy(
            &emu.engine,
            Some(&allow_on(&collection("imported-model"))),
        );
        let refused = emu.call_as(SESSION_KEY_ID, method, &path, body).await;
        assert_refused(
            &format!("{case:?} under a policy scoped to another family"),
            &refused,
            operation,
            &addressed,
            Verdict::DefaultDeny,
        );
    }
}

/// RULE 1, the effect half: the decision stands in FRONT of the work, so a refused create
/// mints nothing — whatever else the request carried.
#[tokio::test]
async fn a_refused_control_plane_create_performs_nothing() {
    let emu = emulator();
    given_session(&emu.engine);
    given_role(&emu.engine, JOB_ROLE, Some(&trust(None)));
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");

    // The guardrail create, read back through the listing that reports it.
    given_role_policy(&emu.engine, Some(&allow_all_but("bedrock:CreateGuardrail")));
    let refused = emu
        .call_as(
            SESSION_KEY_ID,
            "POST",
            "/guardrails",
            Some(json!({
                "name": "denied-guardrail",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
        )
        .await;
    assert_eq!(
        refused.status, 403,
        "case \"a guardrail create\": expected the modelled 403, got {} {:?}",
        refused.status, refused.body
    );
    let listed = emu.call_as(ACCOUNT_KEY_ID, "GET", "/guardrails", None).await;
    assert_eq!(
        listed.status, 200,
        "case \"a guardrail create\": the listing must be served to the account, got {} {:?}",
        listed.status, listed.body
    );
    let reported = listed.get("guardrails").as_array().cloned().unwrap_or_default();
    assert!(
        reported.is_empty(),
        "case \"a guardrail create\": a refused create must leave nothing to list, got \
         {reported:?}"
    );

    // And both job creates, each denied its own action and each read back the way its own
    // jobs are read back ([`job_held`]).
    for create in JOB_CREATES {
        let action = format!("bedrock:{}", create.case);
        given_role_policy(&emu.engine, Some(&allow_all_but(&action)));
        let name = job_name("denied", &create.case.to_lowercase());
        let case = format!("{:?}", create.case);
        let reply = emu
            .call_as(
                SESSION_KEY_ID,
                "POST",
                create.path,
                Some((create.body)(&name, &role_arn)),
            )
            .await;
        assert_eq!(
            reply.status, 403,
            "case {case}: expected the modelled 403, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            !job_held(&emu, create, &case, &name).await,
            "case {case}: a refused create must leave no job behind"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The pass-role decision.
// ---------------------------------------------------------------------------------------

/// One job-creating operation: the create, and how the absence of the job it named is read
/// back.
struct JobCreate {
    case: &'static str,
    path: &'static str,
    body: fn(&str, &str) -> Value,
    /// The operation's own listing and the member it reports its jobs under, where that
    /// listing is implemented; `None` where it is still a scaffolded 501 under its own
    /// issue, and the store the handlers share answers instead ([`job_held`]).
    listing: Option<(&'static str, &'static str)>,
    /// The success status the model declares for the create
    /// (`CreateModelInvocationJob` 200, `CreateEvaluationJob` 202).
    success: u16,
}

/// Both operations this change adds the pass-role decision to.
const JOB_CREATES: &[JobCreate] = &[
    JobCreate {
        case: "CreateModelInvocationJob",
        path: "/model-invocation-job",
        body: invocation_job_body,
        listing: Some(("/model-invocation-jobs", "invocationJobSummaries")),
        success: 200,
    },
    JobCreate {
        case: "CreateEvaluationJob",
        path: "/evaluation-jobs",
        body: evaluation_job_body,
        listing: None,
        success: 202,
    },
];

/// Whether this account holds a job of `name` for `create`.
///
/// Read through the operation's own listing where the model's listing is implemented, and
/// out of the store the handlers share where it is not: `ListEvaluationJobs` is still a
/// scaffolded 501 owned by its own stub issue, and the half of the rule under test here is
/// about what a refused create WROTE, which the store answers whether or not a reader for
/// it exists yet.
async fn job_held(emu: &Emulator, create: &JobCreate, case: &str, name: &str) -> bool {
    if let Some((listing, items)) = create.listing {
        let listed = emu
            .call_as(
                ACCOUNT_KEY_ID,
                "GET",
                &format!("{listing}?nameContains={name}"),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: the listing must be served to the account, got {} {:?}",
            listed.status, listed.body
        );
        let reported = listed.get(items).as_array().cloned().unwrap_or_default();
        return !reported.is_empty();
    }
    let state = bedrock::state::service_state(&*emu.engine).expect("this service's store");
    let store = state.lock();
    store
        .evaluation_job_names
        .contains_key(&(REGION.to_string(), name.to_string()))
}

/// RULE 2: a job create is admitted only where this account's IAM holds the role it passes
/// and that role's trust policy grants Amazon Bedrock the assume this pass would make;
/// anything else is 403 `AccessDeniedException` with the one sentence the denial is stated
/// with, and writes no job.
#[tokio::test]
async fn a_job_create_decides_the_pass_of_the_role_it_hands_bedrock() {
    let emu = emulator();
    let held = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");

    // One row: the trust policy IAM holds for the role, the role ARN the create passes,
    // and whether this account may make that pass.
    struct Pass {
        case: &'static str,
        trust_policy: Option<Value>,
        role_arn: String,
        admitted: bool,
    }
    let rows: Vec<Pass> = vec![
        Pass {
            case: "a role trusted to this service",
            trust_policy: Some(trust(None)),
            role_arn: held.clone(),
            admitted: true,
        },
        Pass {
            case: "a trust narrowed by a condition the pass satisfies",
            trust_policy: Some(trust(Some(json!({
                "StringEquals": { "aws:SourceAccount": ACCOUNT },
            })))),
            role_arn: held.clone(),
            admitted: true,
        },
        Pass {
            // The pass carries the ARN of the job being created as `aws:SourceArn`, so a
            // grant narrowed to this account's Bedrock resources in this Region covers
            // either job create.
            case: "a trust narrowed by the source ARN of this account's jobs",
            trust_policy: Some(trust(Some(json!({
                "StringEquals": { "aws:SourceAccount": ACCOUNT },
                "ArnLike": {
                    "aws:SourceArn": format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:*"),
                },
            })))),
            role_arn: held.clone(),
            admitted: true,
        },
        Pass {
            // The same key, narrowed to a job family NEITHER create mints: the clause is
            // decided and does not hold. Which family each create's source ARN names, and
            // the family-by-family table, is `crate::job_pass_role_source_arn`.
            case: "a trust narrowed to another job family",
            trust_policy: Some(trust(Some(json!({
                "ArnEquals": {
                    "aws:SourceArn":
                        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-customization-job/*"),
                },
            })))),
            role_arn: held.clone(),
            admitted: false,
        },
        Pass {
            case: "a trust narrowed to another account",
            trust_policy: Some(trust(Some(json!({
                "StringEquals": { "aws:SourceAccount": "123456789012" },
            })))),
            role_arn: held.clone(),
            admitted: false,
        },
        Pass {
            case: "a trust granting another service only",
            trust_policy: Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "sagemaker.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            })),
            role_arn: held.clone(),
            admitted: false,
        },
        Pass {
            case: "a trust denying this service beside an allow",
            trust_policy: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                ],
            })),
            role_arn: held.clone(),
            admitted: false,
        },
        Pass {
            case: "a role carrying no trust policy",
            trust_policy: None,
            role_arn: held.clone(),
            admitted: false,
        },
        Pass {
            case: "a role of another account",
            trust_policy: Some(trust(None)),
            role_arn: format!("arn:aws:iam::123456789012:role/{JOB_ROLE}"),
            admitted: false,
        },
        Pass {
            case: "a role this account's IAM does not hold",
            trust_policy: Some(trust(None)),
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/NoSuchJobRole"),
            admitted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        given_role(&emu.engine, JOB_ROLE, row.trust_policy.as_ref());
        for create in JOB_CREATES {
            let name = job_name(&format!("pass{index}"), &create.case.to_lowercase());
            let case = format!("{:?} / {}", row.case, create.case);
            let reply = emu
                .call_as(
                    ACCOUNT_KEY_ID,
                    "POST",
                    create.path,
                    Some((create.body)(&name, &row.role_arn)),
                )
                .await;
            if row.admitted {
                assert_eq!(
                    reply.status, create.success,
                    "case {case}: a pass this account may make must be admitted, got {} {:?}",
                    reply.status, reply.body
                );
                assert!(
                    reply.text("jobArn").contains(&format!(":{ACCOUNT}:")),
                    "case {case}: the admitted create must answer a minted jobArn, got {:?}",
                    reply.body
                );
                continue;
            }
            assert_eq!(
                reply.status, 403,
                "case {case}: a pass this account may not make must be refused 403, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("AccessDeniedException"),
                "case {case}: expected AccessDeniedException, got {:?}",
                reply.error
            );
            assert_eq!(
                reply.get("message"),
                json!(PASS_ROLE_REFUSAL),
                "case {case}: expected {PASS_ROLE_REFUSAL:?}, got {:?}",
                reply.body
            );

            // A denied pass writes nothing: the job it named is held nowhere.
            assert!(
                !job_held(&emu, create, &case, &name).await,
                "case {case}: a denied pass must leave no job behind"
            );
        }
    }
}
