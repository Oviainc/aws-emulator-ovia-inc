// SPDX-License-Identifier: CC0-1.0

//! WHO may reach the custom-model operations of this service, driven over the real wire
//! path (a restJson1 request through the multi-service router into the mounted service),
//! so what is asserted is what a client sees.
//!
//! THE RULE UNDER TEST: Amazon Bedrock takes IAM's authorization decision BEFORE it reads
//! or writes anything, so every operation of the custom-model resource — the create, the
//! read it is tracked through and the listing it appears in
//! (`smithy_query(query="doc",shape="CreateCustomModel")`: "Related APIs GetCustomModel
//! ListCustomModels DeleteCustomModel") — evaluates the identity-based policies in force on
//! the principal the request's credentials name against `bedrock:<OperationName>` on the
//! resource the request addressed. A request no statement allows, or one an explicit `Deny`
//! covers, is refused with the modelled `AccessDeniedException` (`@httpError(403)`) and
//! performs nothing: no model is minted and no stored model is reported. A request a
//! matching `Allow` covers — and the account's own credential, which holds no IAM policies
//! — is served.
//!
//! The tables span what the rule generalises over: the OPERATION (a create, a read
//! addressed by ARN, the same read addressed by name, a listing), the DOCUMENT in force
//! (none at all, another action of this service only, an allow of exactly the action, an
//! allow of the whole service, an explicit deny beside an allow of everything), and the
//! CREDENTIAL (an assumed-role session, the account's own published key). The refusal's
//! sentence is asserted whole, with every part — principal, action, resource, reason —
//! taken from the request and the stored policy.
//!
//! The role, its policy and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test
//! are the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The harness below (the emulator, the signed call, the seeded session and the policy
//! documents) is SHARED with `crate::control_plane_authorization`, which asserts the same
//! rule over the rest of this service's operations: one harness, so two suites cannot drive
//! two different dispatch paths for one rule.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{error_code, label, Reply, ACCOUNT, REGION};

/// The access key id of the account's own published credential, as every other suite in
/// this crate signs with it: a key neither STS nor IAM holds a record for, which is the
/// position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the assumed-role session the policy rows sign with.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
pub(crate) const ROLE: &str = "bedrock-custom-model-caller";
const SESSION: &str = "custom-model-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A `RoleArn` a create naming its artifacts in Amazon S3 must carry — the role Amazon
/// Bedrock would take on, which is not the principal making the call.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockCustomModelImport";

/// A model source inside the member's published form. No Amazon S3 is mounted here, so no
/// object store answers whose bucket it is and the source is not what decides these rows —
/// the authorization decision stands in front of it either way.
const SOURCE_URI: &str = "s3://example-custom-model-artifacts/model/";

/// One emulator: this service beside IAM and STS over ONE shared engine, so the policies
/// and the session the cases seed are the ones those services hold.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

impl Emulator {
    /// One control-plane call signed with `access_key_id`, which is what names the
    /// principal the request speaks for.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(value).to_string());
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|value| error_code(value).to_string())
        });
        Reply {
            status,
            content_type,
            body,
            error,
        }
    }
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
pub(crate) fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them: the identity is the `assumed-role`
/// ARN `sts:GetCallerIdentity` reports, and the role is the IAM entity whose policies are
/// in force on it.
pub(crate) fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEBEDROCKROLE".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
pub(crate) fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// Which verdict a row expects, and — for a refusal — which of IAM's rules reached it.
///
/// Shared with `crate::control_plane_authorization`, which asserts the same rule over the
/// rest of this service's operations: IAM's refusal clauses are one vocabulary, so the two
/// suites read them from one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// IAM permits the request, so the operation answers its own way.
    Served,
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Verdict {
    /// The clause IAM closes the refusal with.
    pub(crate) fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::Served => panic!("a served request carries no refusal clause"),
        }
    }
}

/// A policy allowing `actions` on every resource.
pub(crate) fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy allowing everything and denying `action` — the shape a least-privilege
/// guardrail takes, and the one the refusal must attribute to an explicit deny.
pub(crate) fn allow_all_but(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "*", "Resource": "*" },
            { "Effect": "Deny", "Action": action, "Resource": "*" },
        ]
    })
}

/// One custom-model request: the operation IAM is asked about, the call that makes it, and
/// the resource ARN the refusal must name.
struct Call {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
}

/// Every custom-model operation a request can reach with a credential, one per class the
/// rule generalises over: the create, the read addressed by the model's ARN, the same read
/// addressed by its name, and the listing.
fn calls(model_arn: &str, model_name: &str) -> Vec<Call> {
    let collection = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/*");
    vec![
        Call {
            case: "a create",
            operation: "CreateCustomModel",
            method: "POST",
            path: "/custom-models/create-custom-model".to_string(),
            body: Some(json!({
                "modelName": "authorization-probe",
                "modelSourceConfig": { "s3DataSource": { "s3Uri": SOURCE_URI } },
                "roleArn": SERVICE_ROLE_ARN,
            })),
            resource: collection.clone(),
        },
        Call {
            case: "a read addressed by the model's ARN",
            operation: "GetCustomModel",
            method: "GET",
            path: format!("/custom-models/{}", label(model_arn)),
            body: None,
            // An ARN names the resource itself, so that is what the action is judged on.
            resource: model_arn.to_string(),
        },
        Call {
            case: "a read addressed by the model's name",
            operation: "GetCustomModel",
            method: "GET",
            path: format!("/custom-models/{}", label(model_name)),
            body: None,
            // A bare name is no ARN, so the action reaches the collection.
            resource: collection.clone(),
        },
        Call {
            case: "a listing",
            operation: "ListCustomModels",
            method: "GET",
            path: "/custom-models".to_string(),
            body: None,
            resource: collection,
        },
    ]
}

/// One custom model, created with the account's own credential — the precondition the read
/// rows address.
async fn fixture(emu: &Emulator) -> (String, String) {
    let name = "authorization-fixture";
    let reply = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            "/custom-models/create-custom-model",
            Some(json!({
                "modelName": name,
                "modelSourceConfig": { "s3DataSource": { "s3Uri": SOURCE_URI } },
                "roleArn": SERVICE_ROLE_ARN,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 202,
        "the account's own credential must be served, got {} {:?}",
        reply.status, reply.body
    );
    (reply.text("modelArn"), name.to_string())
}

/// RULE: every custom-model operation evaluates the caller's identity-based policies
/// against `bedrock:<OperationName>` on the resource the request addressed, refusing with
/// 403 `AccessDeniedException` — and IAM's own sentence — what no statement allows or a
/// `Deny` covers.
#[tokio::test]
async fn a_custom_model_operation_is_authorized_against_the_callers_policies() {
    let emu = emulator();
    let (model_arn, model_name) = fixture(&emu).await;
    given_session(&emu.engine);

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
            "an allow of exactly the custom-model actions",
            Some(allow(json!([
                "bedrock:CreateCustomModel",
                "bedrock:GetCustomModel",
                "bedrock:ListCustomModels",
            ]))),
            Verdict::Served,
        ),
    ];

    for (document_case, document, verdict) in &documents {
        given_role_policy(&emu.engine, document.as_ref());
        for call in calls(&model_arn, &model_name) {
            let case = format!("{document_case:?} / {:?}", call.case);
            let reply = emu
                .call_as(SESSION_KEY_ID, call.method, &call.path, call.body.clone())
                .await;
            if *verdict == Verdict::Served {
                assert_ne!(
                    reply.status, 403,
                    "case {case}: a matching Allow must be served, got {} {:?}",
                    reply.status, reply.body
                );
                assert_ne!(
                    reply.error.as_deref(),
                    Some("AccessDeniedException"),
                    "case {case}: a matching Allow must not draw AccessDeniedException, got \
                     {:?}",
                    reply.body
                );
                continue;
            }
            assert_refused(&case, &reply, call.operation, &call.resource, *verdict);
        }
    }

    // The explicit-deny half, with the same calls under a document that allows everything
    // and denies the one action: the refusal must attribute itself to that deny rather than
    // to the default one.
    for call in calls(&model_arn, &model_name) {
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
    }
}

/// The refusal one row expects: 403, the modelled error code, and IAM's own sentence with
/// every part taken from the request and the stored policy.
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
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {}",
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

/// RULE: the decision stands in FRONT of the work, so a refused create mints nothing and a
/// refused read reports nothing — the store is the same after the refusal as before it.
#[tokio::test]
async fn a_refused_custom_model_request_performs_nothing() {
    let emu = emulator();
    given_session(&emu.engine);
    given_role_policy(
        &emu.engine,
        Some(&allow_all_but("bedrock:CreateCustomModel")),
    );

    // Rows differing in the rest of the request, so the refusal is about the identity
    // rather than about the body: a create that would be admitted, one carrying an
    // idempotency token, and one carrying tags.
    let rows: &[(&str, &str, Value)] = &[
        (
            "a create a permitted principal would be served",
            "denied-plain",
            json!({}),
        ),
        (
            "a create carrying an idempotency token",
            "denied-token",
            json!({ "clientRequestToken": "denied-token-1" }),
        ),
        (
            "a create carrying tags",
            "denied-tagged",
            json!({ "modelTags": [{ "key": "team", "value": "platform" }] }),
        ),
    ];

    for (case, name, extra) in rows {
        let mut body = json!({
            "modelName": name,
            "modelSourceConfig": { "s3DataSource": { "s3Uri": SOURCE_URI } },
            "roleArn": SERVICE_ROLE_ARN,
        });
        for (member, value) in extra.as_object().expect("the row's extra members") {
            body[member] = value.clone();
        }
        let reply = emu
            .call_as(
                SESSION_KEY_ID,
                "POST",
                "/custom-models/create-custom-model",
                Some(body),
            )
            .await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected the modelled 403, got {} {:?}",
            reply.status, reply.body
        );

        // Asked with the account's own credential, which IAM permits: nothing was written.
        let listed = emu
            .call_as(
                ACCOUNT_KEY_ID,
                "GET",
                &format!("/custom-models?nameContains={name}"),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must be served to the account, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = listed
            .get("modelSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            summaries.is_empty(),
            "case {case:?}: a refused create must leave nothing to list, got {summaries:?}"
        );
    }
}
