// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service, driven over the real wire path (a restJson1 request through
//! the multi-service router into the mounted service beside IAM and STS on one engine), so
//! what is asserted is what a client sees.
//!
//! RULE. AWS takes IAM's decision in FRONT of the service, so a request is served only
//! where the identity-based policies in force on the principal its credentials name allow
//! `bedrock:<OperationName>` on the resource the request addressed; a request no statement
//! allows, or one an explicit `Deny` covers, is refused with the modelled
//! `AccessDeniedException` (`@httpError(403)`, a service-common error of this model, so
//! every operation declares it) and performs NOTHING.
//!
//! The rows span what the rule generalises over: the OPERATION (a create, a read addressed
//! by an ARN in the path, a listing, another family's create, a delete), the DOCUMENT in
//! force on the principal (none at all, another service's actions only, another action of
//! this service only, an allow of exactly the action, an allow of the whole service, an
//! allow of everything beside an explicit deny — the shape a scoped-down role carries), and
//! the CREDENTIAL (an assumed-role session, the account's own published key). The refusal's
//! sentence is asserted whole, with every part — principal, action, resource, reason — taken
//! from the request and the stored policy, and the resource it names being the one the
//! request ADDRESSED: the ARN where the request carries one, else the collection of the
//! family whose URI space it was sent to.
//!
//! The three tagging operations are judged by this same layer, against the ARN their BODY
//! names; they are asserted where they are REACHABLE, which is beside the Bedrock control
//! plane that claims the paths both models bind
//! (`services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/tests/all/data_automation_tagging.rs`).
//!
//! The decision itself is `service_common::iam_authorization`, shared with every other
//! service that authorizes; what this suite pins is that this service takes it, for every
//! operation, before anything is minted or written.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice as bda;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// The Region every call signs for, and the account the emulator mints ARNs under.
const REGION: &str = "us-west-2";
const ACCOUNT: &str = "000000000000";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the assumed-role session the policy rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "bedrock-data-automation-caller";
const SESSION: &str = "bda-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// One emulator: this service beside IAM and STS over ONE shared engine, so the policies
/// and the session the cases seed are the ones those services hold.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
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

/// What one call answered: the status, the error code (from the header or the body) and the
/// decoded body.
struct Reply {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Emulator {
    /// One call signed with `access_key_id`, which is what names the principal the request
    /// speaks for.
    async fn call_as(
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
            .uri(format!(
                "https://bedrock-data-automation.{REGION}.amazonaws.com{path}"
            ))
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
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(&json!({ "__type": value })));
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .map(|_| error_code(&body))
                .filter(|code| code != "<no __type>")
        });
        Reply {
            status,
            error,
            body,
        }
    }
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
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
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEBDAROLE".to_string(),
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

/// Attach `document` to the role as its inline policy, replacing whatever it carried — what
/// `iam:PutRolePolicy` does. `None` takes every policy off it, as `iam:DeleteRolePolicy`
/// does, which is the shape a role created with a trust policy ALONE carries.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
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

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy allowing everything beside an explicit deny of `actions` — the shape a
/// scoped-down role carries, where the `Deny` beats any `Allow` the principal also holds.
fn allow_all_but(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "*", "Resource": "*" },
            { "Effect": "Deny", "Action": actions, "Resource": "*" },
        ]
    })
}

/// Which verdict a row expects, and — for a refusal — which of IAM's rules reached it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// IAM permits the request, so the operation answers its own way.
    Served,
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Verdict {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::Served => panic!("a served request carries no refusal clause"),
        }
    }
}

/// The collection of `resource_type` in this account and Region: the ARN IAM evaluates an
/// action against when the request names no single resource of the family it addressed.
fn collection(resource_type: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/*")
}

/// One request a credential can reach this service with: the operation IAM is asked about,
/// the call that makes it, and the resource the refusal must name.
struct Call {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
}

/// A blueprint created with the account's own credential, which every row that addresses a
/// stored resource is then aimed at. Returns its ARN.
async fn given_blueprint(emulator: &Emulator, name: &str) -> String {
    let reply = emulator
        .call_as(
            ACCOUNT_KEY_ID,
            "PUT",
            "/blueprints/",
            Some(json!({
                "blueprintName": name,
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "the fixture blueprint must be created, got {} with {:?}",
        reply.status, reply.body
    );
    let blueprint = reply
        .body
        .get("blueprint")
        .cloned()
        .expect("the create answers a blueprint");
    string_at(&blueprint, "blueprintArn", "fixture")
}

/// Every class of request a credential can reach this service with, one row per thing the
/// rule generalises over. `suffix` keeps two rows from colliding on a resource name.
fn calls(blueprint_arn: &str, suffix: &str) -> Vec<Call> {
    vec![
        Call {
            case: "a create, which names no single resource",
            operation: "CreateBlueprint",
            method: "PUT",
            path: "/blueprints/".to_string(),
            body: Some(json!({
                "blueprintName": format!("authorization-{suffix}"),
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
            })),
            resource: collection("blueprint"),
        },
        Call {
            case: "a read addressed by an ARN in the path",
            operation: "GetBlueprint",
            method: "POST",
            path: format!("/blueprints/{}/", encode_label(blueprint_arn)),
            body: Some(json!({})),
            resource: blueprint_arn.to_string(),
        },
        Call {
            case: "a listing, which names no single resource",
            operation: "ListBlueprints",
            method: "POST",
            path: "/blueprints/".to_string(),
            body: Some(json!({})),
            resource: collection("blueprint"),
        },
        Call {
            case: "a project create, another family's collection",
            operation: "CreateDataAutomationProject",
            method: "PUT",
            path: "/data-automation-projects/".to_string(),
            body: Some(json!({
                "projectName": format!("authorization-{suffix}"),
                "standardOutputConfiguration": {},
            })),
            resource: collection("data-automation-project"),
        },
        Call {
            case: "a delete addressed by an ARN in the path",
            operation: "DeleteBlueprint",
            method: "DELETE",
            path: format!("/blueprints/{}/", encode_label(blueprint_arn)),
            body: None,
            resource: blueprint_arn.to_string(),
        },
    ]
}

/// RULE: every operation of this service is authorized before it reads or writes anything —
/// served only where the policies in force on the caller's principal allow
/// `bedrock:<OperationName>` on the resource the request addressed, and otherwise refused
/// 403 `AccessDeniedException` with IAM's own sentence.
#[tokio::test]
async fn every_operation_is_authorized_against_the_resource_the_request_addresses() {
    // (case, the document in force on the role, the verdict every call then gets)
    let documents: Vec<(&str, Option<Value>, Verdict)> = vec![
        ("no policy at all", None, Verdict::DefaultDeny),
        (
            "another service's actions only",
            Some(allow(json!("s3:*"))),
            Verdict::DefaultDeny,
        ),
        (
            "one other action of this service only",
            Some(allow(json!("bedrock:GetDataAutomationProject"))),
            Verdict::DefaultDeny,
        ),
        (
            "every action of this service",
            Some(allow(json!("bedrock:*"))),
            Verdict::Served,
        ),
        (
            "every action of every service",
            Some(allow(json!("*"))),
            Verdict::Served,
        ),
        (
            "everything but an explicit deny of this service",
            Some(allow_all_but(json!("bedrock:*"))),
            Verdict::ExplicitDeny,
        ),
    ];

    for (index, (document_case, document, verdict)) in documents.into_iter().enumerate() {
        let emulator = emulator();
        given_session(&emulator.engine);
        given_role_policy(&emulator.engine, document.as_ref());
        let suffix = format!("{index}");
        let blueprint_arn = given_blueprint(&emulator, &format!("fixture-{suffix}")).await;

        for call in calls(&blueprint_arn, &suffix) {
            let case = format!("{document_case} / {}", call.case);
            let reply = emulator
                .call_as(
                    SESSION_KEY_ID,
                    call.method,
                    &call.path,
                    call.body.clone(),
                )
                .await;
            match verdict {
                Verdict::Served => {
                    assert!(
                        reply.status < 400,
                        "case {case:?}: expected the request to be served, got {} with {:?}",
                        reply.status,
                        reply.body
                    );
                }
                refused => {
                    assert_eq!(
                        reply.status, 403,
                        "case {case:?}: expected 403, got {} with {:?}",
                        reply.status, reply.body
                    );
                    assert_eq!(
                        reply.error.as_deref(),
                        Some("AccessDeniedException"),
                        "case {case:?}: expected AccessDeniedException, got {:?}",
                        reply.error
                    );
                    let action = format!("bedrock:{}", call.operation);
                    let expected = format!(
                        "User: {} is not authorized to perform: {action} on resource: {} {}",
                        session_arn(),
                        call.resource,
                        refused.reason(&action)
                    );
                    let message = reply
                        .body
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert_eq!(
                        message, expected,
                        "case {case:?}: expected IAM's wording, got {message:?}"
                    );
                }
            }
        }
    }
}

/// RULE: a refused call performs NOTHING — no blueprint is minted, and the resource the
/// refused call named is left exactly as it was.
#[tokio::test]
async fn a_refused_call_mints_nothing_and_changes_nothing() {
    let emulator = emulator();
    given_session(&emulator.engine);
    given_role_policy(&emulator.engine, None);
    let blueprint_arn = given_blueprint(&emulator, "untouched").await;

    // (case, the call the no-permission session makes)
    let refused = calls(&blueprint_arn, "nothing");
    for call in &refused {
        let reply = emulator
            .call_as(SESSION_KEY_ID, call.method, &call.path, call.body.clone())
            .await;
        assert_eq!(
            reply.status, 403,
            "case {:?}: expected 403, got {} with {:?}",
            call.case, reply.status, reply.body
        );
    }

    // The account's own credential still reads the fixture back unchanged, and the store
    // holds exactly the one blueprint it created: nothing the refused creates named exists.
    let read = emulator
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            &format!("/blueprints/{}/", encode_label(&blueprint_arn)),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the fixture survives\": expected 200, got {} with {:?}",
        read.status, read.body
    );
    let listed = emulator
        .call_as(ACCOUNT_KEY_ID, "POST", "/blueprints/", Some(json!({})))
        .await;
    let count = listed
        .body
        .get("blueprints")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        count, 1,
        "case \"a refused create mints nothing\": expected the one fixture blueprint, got \
         {:?}",
        listed.body
    );
}

/// RULE: the account's own published credential is not a principal the IAM store holds
/// policies for — the position AWS account root credentials are in — so a request signed
/// with it is served whatever the role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bounded_by_the_roles_policies() {
    let emulator = emulator();
    given_session(&emulator.engine);
    given_role_policy(&emulator.engine, Some(&allow_all_but(json!("bedrock:*"))));
    let blueprint_arn = given_blueprint(&emulator, "ambient").await;

    for call in calls(&blueprint_arn, "ambient") {
        let reply = emulator
            .call_as(ACCOUNT_KEY_ID, call.method, &call.path, call.body.clone())
            .await;
        assert!(
            reply.status < 400,
            "case {:?}: expected the account's own credential to be served, got {} with {:?}",
            call.case,
            reply.status,
            reply.body
        );
    }
}
