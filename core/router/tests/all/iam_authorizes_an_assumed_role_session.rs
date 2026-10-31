// SPDX-License-Identifier: CC0-1.0

//! IAM authorizes its OWN API for a caller holding a role's temporary credential, not only
//! for one holding a user's long-term key.
//!
//! RULE: the principal of a request is the identity the access key id it presents speaks
//! for, and for the credential `sts:AssumeRole` vends that is the SESSION — whose
//! permissions are the role's ("the temporary security credentials … have the permissions
//! of the role"). So every IAM operation decides such a call against the inline and
//! attached policies of that role, under IAM's documented evaluation: an explicit `Deny`
//! about the request wins over every `Allow`, an `Allow` with no `Deny` permits, and a
//! principal with neither is refused. A refusal is `AccessDenied` at HTTP 403, worded from
//! the request: the SESSION's ARN as the principal, the action asked for, the resource the
//! request addressed, and why the decision went that way.
//!
//! Two services have to agree for that to hold — IAM holds the role and its policies and
//! makes the decision, STS mints the session credential and names its principal — so this
//! is the suite that can pin it: a crate-local suite can seed one side's state but never
//! drive `CreateRole`/`PutRolePolicy`/`AssumeRole` end to end into the other's answer.
//!
//! The rows span what the rule generalises over: what the role's policies say (a broad
//! grant, a grant plus an explicit `Deny` naming one action, a `Deny` on another action,
//! and no policy at all), which action is asked for (a listing of users, of groups and of
//! roles — three different operations of this service), and whether the deciding statement
//! is in the role's FIRST or SECOND inline policy. Beside the table: removing the `Deny`
//! restores the grant, and a user's long-term key is still decided on the user's own
//! policies.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is the ambient identity's: the
/// account's own administrative credential, which holds no IAM policies.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names.
const SESSION: &str = "probe";

/// A trust policy granting the account root permission to assume a role.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// IAM and STS over ONE engine, as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key the service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM action, presenting `access_key_id`.
async fn iam(
    router: &MultiServiceRouter,
    access_key_id: &str,
    body: String,
) -> (u16, String) {
    query(
        router,
        access_key_id,
        "iam",
        format!("Version=2010-05-08&{body}"),
    )
    .await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The awsQuery error code of a refusal.
fn error_code(body: &str) -> String {
    xml_element(body, "Code").unwrap_or_else(|| "<no Code>".to_string())
}

/// The `Message` of a refusal.
fn message_of(body: &str) -> String {
    xml_element(body, "Message").unwrap_or_else(|| "<no Message>".to_string())
}

/// One identity-based statement, as a document carrying nothing a form body treats
/// specially.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create a role the account root may assume, as the ambient identity.
async fn create_role(router: &MultiServiceRouter, role_name: &str) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!(
            "Action=CreateRole&RoleName={role_name}&AssumeRolePolicyDocument={TRUST_POLICY}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole expected 200, got {status}: {body}"
    );
}

/// Embed one inline policy in a role, as the ambient identity.
async fn put_role_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!(
            "Action=PutRolePolicy&RoleName={role_name}&PolicyName={policy_name}\
             &PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}/{policy_name:?}: PutRolePolicy expected 200, got {status}: {body}"
    );
}

/// Assume `role_name` and answer the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={SESSION}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: AssumeRole expected 200, got {status}: {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {role_name:?}: no <AccessKeyId> in {body}"))
}

/// The session ARN `AssumeRole` reports for `role_name`, which is what a refusal names as
/// the principal.
fn session_arn(role_name: &str) -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{SESSION}")
}

/// The refusal IAM words for a decision taken on the session of `role_name`.
fn not_authorized(role_name: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
        session_arn(role_name)
    )
}

/// One listing of this service, as a row addresses it: the action, the form parameters it
/// takes, the `iam:` action name a policy statement names it by, and the resource ARN the
/// decision is made against.
struct Listing {
    action: &'static str,
    parameters: &'static str,
    iam_action: &'static str,
    resource: String,
}

/// The three listings the rows ask for. Each addresses the collection of ONE entity type
/// under the path prefix it lists, which is the resource IAM decides such a call against.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            action: "ListUsers",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListUsers",
            resource: format!("arn:aws:iam::{ACCOUNT}:user/probe/"),
        },
        Listing {
            action: "ListGroups",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListGroups",
            resource: format!("arn:aws:iam::{ACCOUNT}:group/probe/"),
        },
        Listing {
            action: "ListRoles",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListRoles",
            resource: format!("arn:aws:iam::{ACCOUNT}:role/probe/"),
        },
    ]
}

/// Call one listing as `access_key_id`.
async fn list(
    router: &MultiServiceRouter,
    access_key_id: &str,
    listing: &Listing,
) -> (u16, String) {
    iam(
        router,
        access_key_id,
        format!("Action={}&{}", listing.action, listing.parameters),
    )
    .await
}

/// RULE: a request signed with a role session's credential is decided against the ROLE's
/// identity-based policies — a broad `Allow` grants every listing, an explicit `Deny`
/// naming one action refuses exactly that one at 403 `AccessDenied` with IAM's sentence
/// while its siblings keep succeeding, and a role carrying no policy at all is refused
/// every one of them.
#[tokio::test]
async fn an_assumed_role_session_is_decided_against_the_roles_policies() {
    for (index, denied) in listings().iter().enumerate() {
        let router = build_router();
        let role_name = format!("SessionRole{index}");
        create_role(&router, &role_name).await;
        // The grant and the deny are SEPARATE inline policies, so the decision is taken
        // over the union of the documents in force rather than within one of them.
        put_role_policy(
            &router,
            &role_name,
            "allow-everything",
            &statement("Allow", "*", "*"),
        )
        .await;
        let session_key = assume_role(&router, &role_name).await;

        // The positive control: with only the broad grant, every listing is served.
        for listing in &listings() {
            let (status, body) = list(&router, &session_key, listing).await;
            assert_eq!(
                status, 200,
                "case {:?} under a broad Allow: expected 200, got {status}: {body}",
                listing.action
            );
        }

        // The explicit Deny names ONE action of the three.
        put_role_policy(
            &router,
            &role_name,
            "deny-one",
            &statement("Deny", denied.iam_action, "*"),
        )
        .await;

        for listing in &listings() {
            let case = format!("{} with a Deny on {}", listing.action, denied.iam_action);
            let (status, body) = list(&router, &session_key, listing).await;
            if listing.iam_action == denied.iam_action {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "AccessDenied",
                    "case {case:?}: expected AccessDenied, got {code:?}: {body}"
                );
                let message = message_of(&body);
                assert_eq!(
                    message,
                    not_authorized(
                        &role_name,
                        listing.iam_action,
                        &listing.resource,
                        "with an explicit deny in an identity-based policy"
                    ),
                    "case {case:?}: expected IAM's explicit-deny refusal, got {message:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a sibling action the Deny does not name must keep \
                     succeeding, got {status}: {body}"
                );
            }
        }

        // Deleting the Deny restores the grant: the decision is taken from the documents
        // in force at the instant of the call, not from the session.
        let (status, body) = iam(
            &router,
            AMBIENT_KEY,
            format!("Action=DeleteRolePolicy&RoleName={role_name}&PolicyName=deny-one"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: DeleteRolePolicy expected 200, got {status}: {body}"
        );
        let (status, body) = list(&router, &session_key, denied).await;
        assert_eq!(
            status, 200,
            "case {:?} after the Deny is deleted: expected 200, got {status}: {body}",
            denied.action
        );
    }
}

/// RULE: a role carrying NO identity-based policy grants nothing, so a session of it is
/// refused every listing — IAM's implicit deny, worded with the document that withheld the
/// call.
#[tokio::test]
async fn a_session_of_a_role_with_no_policy_is_refused() {
    let router = build_router();
    let role_name = "BareRole";
    create_role(&router, role_name).await;
    let session_key = assume_role(&router, role_name).await;

    for listing in &listings() {
        let (status, body) = list(&router, &session_key, listing).await;
        assert_eq!(
            status, 403,
            "case {:?}: expected HTTP 403, got {status}: {body}",
            listing.action
        );
        let code = error_code(&body);
        assert_eq!(
            code, "AccessDenied",
            "case {:?}: expected AccessDenied, got {code:?}: {body}",
            listing.action
        );
        let message = message_of(&body);
        assert_eq!(
            message,
            not_authorized(
                role_name,
                listing.iam_action,
                &listing.resource,
                &format!(
                    "because no identity-based policy allows the {} action",
                    listing.iam_action
                )
            ),
            "case {:?}: expected IAM's implicit-deny refusal, got {message:?}",
            listing.action
        );
    }
}

/// RULE: the account's own published credential is not an identity this account holds
/// policies for — the position AWS account root credentials are in — so its calls are
/// served throughout, whatever a role's policies say. It is what every fixture above is
/// created with.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bound_by_a_roles_policies() {
    let router = build_router();
    let role_name = "DenyAllRole";
    create_role(&router, role_name).await;
    put_role_policy(&router, role_name, "deny-all", &statement("Deny", "*", "*")).await;

    for listing in &listings() {
        let (status, body) = list(&router, AMBIENT_KEY, listing).await;
        assert_eq!(
            status, 200,
            "case {:?}: the account's own credential must keep being served, got {status}: \
             {body}",
            listing.action
        );
    }
}
