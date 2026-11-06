// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY-based half of the `execute-api` data plane's
//! authorizer: what a deployed stage answers a request signed with credentials whose
//! principal is not allowed `execute-api:Invoke`.
//!
//! THE RULE UNDER TEST: a request to a method that AUTHENTICATES a caller
//! (`authorizationType` `AWS_IAM`) is served only where the identity-based policies in
//! force on the principal the presented credentials speak for — or the api's own resource
//! policy, since either grant suffices — allow `execute-api:Invoke` on the method's own ARN
//! (`arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{verb}/{path}`); every other
//! request is refused `403` with the IAM authorization sentence, `x-amzn-ErrorType:
//! AccessDeniedException`, and the integration never reached. A credential that names no
//! principal the account holds policies for is one this emulator cannot judge, and a grant
//! it cannot read grounds no refusal.
//!
//! WHY the session is the case that matters: an identity pool hands short-lived,
//! deliberately under-privileged credentials of its authenticated or unauthenticated role
//! to clients it does not trust, so a role carrying no `execute-api` grant must reach no
//! `AWS_IAM` method of a deployed stage. `cognito-identity:GetCredentialsForIdentity`,
//! `sts:AssumeRole` and `sts:AssumeRoleWithWebIdentity` all mint that credential as an
//! assumed-role session of the same registry, which is the one principal form the rows
//! below seed.
//!
//! The role, its policies, the session and the long-term access key live in the IAM and STS
//! stores of the SAME shared engine — the stores those services' own handlers write — so
//! the documents under test are the ones `iam:PutRolePolicy` stores and the principal a
//! refusal blames is the one `sts:GetCallerIdentity` answers for the key the request signed
//! with. Everything about the api is written through this service's control plane
//! (`CreateRestApi`, `CreateResource`, `PutMethod`, `PutIntegration`, `CreateDeployment`,
//! `UpdateRestApi`) and every case drives the real wire path — an HTTP request through the
//! multi-service router to the api's own invoke host — so what is asserted is what a client
//! holding those credentials sees.
//!
//! The tables span what the rule generalises over: the document in force on the caller
//! (none at all, the exact action on the exact method, the action in another case, every
//! action of the service, another service's action, a grant scoped to another stage, verb
//! or path, a wildcard over the api's methods, an explicit deny beside a grant of
//! everything), the PRINCIPAL the credential speaks for (an assumed-role session, an IAM
//! user's long-term key, the account's own published credential), the api's own resource
//! policy beside the caller's policies (both directions of "either grant suffices"), and
//! the method's own `authorizationType` (`AWS_IAM`, which authenticates a caller, and
//! `NONE`, which authenticates nobody and so reads no identity policy at all).
//!
//! The gate under test is `src/service/invoke/execute_api_authorization.rs`, whose own unit
//! tests pin how the two grants combine over a table of documents, and which
//! `core/router/tests/all/apigateway_iam_authorized_invoke.rs` drives through the real vend
//! across four services. What is added here is the END-TO-END reading of it inside THIS
//! crate, over the rows that suite does not carry: the action in another case, a grant scoped
//! to another stage, verb or path, an IAM user's long-term key, the api's own resource policy
//! in both directions, and a method that authenticates nobody.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::execute_api_data_plane::{integration_response, invoke, json_template, method_response};
use crate::execute_api_data_plane::{deploy, put_integration, put_method};
use crate::rest_api_lifecycle::{resource_under, string_member, Gateway};

/// The Region the api is created in, which the shared control-plane harness scopes its
/// requests to: the Region of the method ARN a statement names and a refusal reports.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "s";

/// The resource the method under test is put on.
const RESOURCE_PATH: &str = "whoami";

/// The one value the `MOCK` integration's response template carries — what proves a request
/// reached the integration, so a refusal must never carry it.
const NONCE: &str = "d41d8c";

/// The role the seeded session is a session of: the identity pool's own role, which carries
/// whatever each row puts on it and nothing else.
const ROLE: &str = "cognito-pool-guest";

/// The session name `cognito-identity:GetCredentialsForIdentity` vends a role session
/// under, which is what the session's own principal ARN spells after the role.
const SESSION: &str = "CognitoIdentityCredentials";

/// The access key id of that session: the credential a vend hands a client, which the
/// request below signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEVENDED001";

/// The IAM user whose LONG-TERM access key the second principal form signs with.
const USER: &str = "stage-client";

/// That key's id.
const USER_KEY_ID: &str = "AKIAEXAMPLELONGTERM1";

/// An access key id neither store holds a record for: the position the account's own
/// published credential is in.
const ACCOUNT_KEY_ID: &str = "AKIDTESTONLY";

/// The name of the inline policy a row attaches to the role.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: u64 = 3_600;

/// The IAM action an invoke of a deployed method asks for.
const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The body the method's `MOCK` integration answers with.
fn served_body() -> String {
    format!("{{\"m\":\"{NONCE}\"}}")
}

/// The ARN the account holds the seeded role under.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// The principal the seeded session speaks for, as `sts:GetCallerIdentity` reports it for
/// its credential — what a refusal of a request signed with it must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the seeded IAM user under, which is both the principal a
/// refusal names for its long-term key and the identity whose policies that request is
/// judged on.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER}",
        ::engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, exactly as
/// `iam:CreateRole` and a vend of the pool's role record them: the session lives in the STS
/// registry every session this emulator mints goes through, carrying the role whose
/// policies a request signed with it is judged on.
fn given_session(gateway: &Gateway) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(gateway.engine()).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: sts::role_id(&role_arn),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            // The role's trust policy, which is what a vend of its credentials is
            // authorized by; nothing here is an identity-based grant.
            assume_role_policy_document: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Federated": "cognito-identity.amazonaws.com" },
                        "Action": "sts:AssumeRoleWithWebIdentity",
                    }]
                })
                .to_string(),
            ),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let identity = sts::Identity {
        user_id: format!("{}:{SESSION}", sts::role_id(&role_arn)),
        account: ::engine::emulator_account_id().to_string(),
        arn: session_arn(),
    };
    // The vend as STS asked for it, built through the registry's own constructor, so what
    // is recorded here is exactly what a mint records. No session policy scopes it: what
    // the credential may do is the role's own policies, which is what each row puts on it.
    let vend = sts::NewSession {
        role_arn: Some(role_arn),
        ..sts::NewSession::new(
            identity,
            sts::SessionKind::AssumedRole,
            i32::try_from(SESSION_SECONDS).unwrap_or(i32::MAX),
        )
    };
    let sessions = sts::service_state(gateway.engine()).expect("the STS store");
    sessions
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the vended session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record them.
fn given_user(gateway: &Gateway) {
    let store = iam::service_state(gateway.engine()).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEXAMPLELONGTERM1".to_string(),
            arn: user_arn(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
        },
    );
}

/// Attach `document` to `entity` as its one inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` and `iam:PutUserPolicy` do. `None` takes every policy off it, as
/// the matching delete does.
fn given_policy(gateway: &Gateway, entity: Entity, document: Option<&Value>) {
    let store = iam::service_state(gateway.engine()).expect("the IAM store");
    let mut store = store.lock();
    let (policies, key) = match entity {
        Entity::Role => (&mut store.role_inline_policies, iam::name_key(ROLE)),
        Entity::User => (&mut store.user_inline_policies, iam::name_key(USER)),
    };
    match document {
        None => {
            policies.remove(&key);
        }
        Some(document) => {
            policies.insert(
                key,
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

/// Which kind of identity a document is attached to.
#[derive(Debug, Clone, Copy)]
enum Entity {
    /// The role the seeded session is of.
    Role,
    /// The IAM user the seeded long-term key belongs to.
    User,
}

/// An identity-based policy allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }]
    })
}

/// One api with a deployed `MOCK` method, as the control plane writes it.
struct Deployed {
    /// The api id.
    api: String,
    /// The api's root resource id.
    root: String,
}

impl Deployed {
    /// An api carrying `GET /whoami` on an `AWS_IAM` method answered by a `MOCK`
    /// integration, deployed to [`STAGE`].
    async fn stand_up(gateway: &Gateway, name: &str) -> Self {
        let (api, root) = crate::rest_api_lifecycle::create_api(gateway, name).await;
        let deployed = Self { api, root };
        deployed
            .mock_method(gateway, RESOURCE_PATH, "AWS_IAM")
            .await;
        deployed
    }

    /// One `GET` resource under the root on a `MOCK` integration answering
    /// [`served_body`], whose method carries `authorization_type`; answers its path part.
    ///
    /// Deployed, because what the data plane serves is read out of the DEPLOYMENT.
    async fn mock_method(
        &self,
        gateway: &Gateway,
        path_part: &str,
        authorization_type: &str,
    ) -> String {
        let resource = resource_under(gateway, &self.api, &self.root, path_part).await;
        put_method(
            gateway,
            &self.api,
            &resource,
            "GET",
            json!({ "authorizationType": authorization_type }),
        )
        .await;
        put_integration(gateway, &self.api, &resource, "GET", json!({ "type": "MOCK" })).await;
        method_response(gateway, &self.api, &resource, "GET", "200", json!({})).await;
        integration_response(
            gateway,
            &self.api,
            &resource,
            "GET",
            "200",
            json!({ "responseTemplates": json_template(&served_body()) }),
        )
        .await;
        deploy(gateway, &self.api, STAGE).await;
        path_part.to_owned()
    }

    /// The ARN of the deployed `GET` on `path_part`: the resource an identity-based
    /// statement names the method by, and the one a refusal reports.
    fn method_arn(&self, path_part: &str) -> String {
        self.execute_api_arn(&format!("{STAGE}/GET/{path_part}"))
    }

    /// An `execute-api` ARN of this api, naming `resource` within it: the account comes
    /// from the emulator's own account contract, as every other ARN here does.
    fn execute_api_arn(&self, resource: &str) -> String {
        format!(
            "arn:aws:execute-api:{REGION}:{account}:{api}/{resource}",
            account = ::engine::emulator_account_id(),
            api = self.api,
        )
    }

    /// `UpdateRestApi` replacing `/policy` with `document`, then `CreateDeployment` so the
    /// stage serves the api as it now stands.
    async fn set_policy(&self, gateway: &Gateway, case: &str, document: &Value) {
        let patch = json!({
            "patchOperations": [
                { "op": "replace", "path": "/policy", "value": document.to_string() }
            ]
        });
        let patched = gateway
            .call("PATCH", &format!("/restapis/{}", self.api), Some(patch))
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected UpdateRestApi to answer 200, got {} with {:?}",
            patched.status, patched.body
        );
        let read = gateway
            .call("GET", &format!("/restapis/{}", self.api), None)
            .await;
        assert_eq!(
            string_member(&read.body, "policy", case),
            document.to_string(),
            "case {case}: expected GetRestApi to report the policy that was set"
        );
        deploy(gateway, &self.api, STAGE).await;
    }
}

/// A SigV4 credential scope naming `access_key_id`, as a request signed with that
/// credential carries it. The signature itself is not verified by this plane (the whole
/// argument is in `src/service.rs`), so what a row varies is the KEY it names.
fn credential(access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/execute-api/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// One answered invoke of `path_part`, signed with `access_key_id`.
async fn invoke_as(
    gateway: &Gateway,
    api: &str,
    access_key_id: &str,
    path_part: &str,
) -> crate::execute_api_data_plane::Invoked {
    let scope = credential(access_key_id);
    invoke(
        gateway,
        api,
        "GET",
        &format!("/{STAGE}/{path_part}"),
        &[("authorization", scope.as_str())],
    )
    .await
}

/// The `Message` member of a refusal body — the member API Gateway answers an authorization
/// refusal with.
fn authorization_message(body: &str, case: &str) -> String {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| panic!("case {case}: expected a JSON body, got {body:?}"));
    parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Message member, got {body:?}"))
        .to_owned()
}

/// Assert the invoke was SERVED the integration's own body.
fn assert_served(case: &str, answered: &crate::execute_api_data_plane::Invoked) {
    assert_eq!(
        answered.status, 200,
        "case {case}: expected the integration's 200, got {} with {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.body,
        served_body(),
        "case {case}: expected the integration's body, got {:?}",
        answered.body
    );
}

/// Assert the invoke was refused as unauthorized: `403`, the `AccessDeniedException` class,
/// the IAM sentence naming the principal, the action and the method ARN, and nothing of the
/// integration's body — a refused request never reached it.
fn assert_refused(
    case: &str,
    answered: &crate::execute_api_data_plane::Invoked,
    principal: &str,
    method_arn: &str,
) {
    assert_eq!(
        answered.status, 403,
        "case {case}: expected 403, got {} with {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.header("x-amzn-errortype"),
        Some("AccessDeniedException"),
        "case {case}: expected the AccessDeniedException class, got {:?}",
        answered.header("x-amzn-errortype")
    );
    let message = authorization_message(&answered.body, case);
    assert_eq!(
        message,
        format!(
            "User: {principal} is not authorized to perform: {INVOKE_ACTION} on resource: \
             {method_arn}"
        ),
        "case {case}: expected the {INVOKE_ACTION} authorization sentence, got {message:?}"
    );
    assert!(
        !answered.body.contains(NONCE),
        "case {case}: a refused request must carry nothing of the integration's body, got {:?}",
        answered.body
    );
}

/// The documents in force on the caller decide the invoke: the grant has to cover the
/// action AND the method the request addresses.
#[tokio::test]
async fn the_callers_own_policies_decide_an_aws_iam_invoke() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "identity-policy-rows").await;
    given_session(&gateway);
    let method = deployed.method_arn(RESOURCE_PATH);
    let api_methods = deployed.execute_api_arn("*");
    let another_stage = deployed.execute_api_arn(&format!("other/GET/{RESOURCE_PATH}"));
    let another_verb = deployed.execute_api_arn(&format!("{STAGE}/POST/{RESOURCE_PATH}"));
    let another_path = deployed.execute_api_arn(&format!("{STAGE}/GET/elsewhere"));
    let denied_beside_a_grant = json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "execute-api:*", "Resource": "*" },
            { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": method },
        ]
    });
    // Rows differ along the document in force on the role the credential is a session of:
    // what it allows, which resource it scopes the grant to, and an explicit deny beside a
    // grant of everything.
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, false),
        ("the action on this very method", Some(allow(INVOKE_ACTION, &method)), true),
        // IAM compares an action name case-insensitively, so a document spelling it in
        // another case grants the same action.
        (
            "the action in another case",
            Some(allow("EXECUTE-API:INVOKE", &method)),
            true,
        ),
        (
            "every action of the service, on every resource",
            Some(allow("execute-api:*", "*")),
            true,
        ),
        (
            "the action on every method of the api",
            Some(allow(INVOKE_ACTION, &api_methods)),
            true,
        ),
        (
            "another service's action",
            Some(allow("s3:GetObject", "*")),
            false,
        ),
        (
            "the invalidation action on this method",
            Some(allow("execute-api:InvalidateCache", &method)),
            false,
        ),
        (
            "the action on the same method of another stage",
            Some(allow(INVOKE_ACTION, &another_stage)),
            false,
        ),
        (
            "the action on another verb of the same path",
            Some(allow(INVOKE_ACTION, &another_verb)),
            false,
        ),
        (
            "the action on another path of the same stage",
            Some(allow(INVOKE_ACTION, &another_path)),
            false,
        ),
        (
            "an explicit deny beside a grant of everything",
            Some(denied_beside_a_grant),
            false,
        ),
    ];
    for (case, document, served) in &rows {
        given_policy(&gateway, Entity::Role, document.as_ref());
        let answered = invoke_as(&gateway, &deployed.api, SESSION_KEY_ID, RESOURCE_PATH).await;
        if *served {
            assert_served(case, &answered);
        } else {
            assert_refused(case, &answered, &session_arn(), &method);
        }
    }
}

/// Which PRINCIPAL the credential speaks for decides whose policies are read and who a
/// refusal names: the assumed-role session for a vended credential, the IAM user for a
/// long-term key, and nobody the account holds policies for — which this emulator cannot
/// judge and so does not refuse — for its own published credential.
#[tokio::test]
async fn the_principal_the_credential_speaks_for_decides_whose_policies_are_read() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "identity-policy-principals").await;
    given_session(&gateway);
    given_user(&gateway);
    let method = deployed.method_arn(RESOURCE_PATH);
    // Neither identity carries a grant: the session and the user are both refused, each
    // named by its own ARN, while the account's own credential is served.
    given_policy(&gateway, Entity::Role, None);
    given_policy(&gateway, Entity::User, None);
    let refused: &[(&str, &str, String)] = &[
        ("an assumed-role session with no grant", SESSION_KEY_ID, session_arn()),
        ("an IAM user's long-term key with no grant", USER_KEY_ID, user_arn()),
    ];
    for (case, key_id, principal) in refused {
        let answered = invoke_as(&gateway, &deployed.api, key_id, RESOURCE_PATH).await;
        assert_refused(case, &answered, principal, &method);
    }
    let unjudged = invoke_as(&gateway, &deployed.api, ACCOUNT_KEY_ID, RESOURCE_PATH).await;
    assert_served("the account's own published credential", &unjudged);

    // Granted on each identity in turn, the SAME request is served: the document that
    // decides it is the one in force on the principal the key names, and not the other's.
    given_policy(&gateway, Entity::Role, Some(&allow(INVOKE_ACTION, &method)));
    let granted = invoke_as(&gateway, &deployed.api, SESSION_KEY_ID, RESOURCE_PATH).await;
    assert_served("the session's role granted the invoke", &granted);
    let still_refused = invoke_as(&gateway, &deployed.api, USER_KEY_ID, RESOURCE_PATH).await;
    assert_refused(
        "the user, while only the role is granted",
        &still_refused,
        &user_arn(),
        &method,
    );
    given_policy(&gateway, Entity::User, Some(&allow(INVOKE_ACTION, &method)));
    let user_granted = invoke_as(&gateway, &deployed.api, USER_KEY_ID, RESOURCE_PATH).await;
    assert_served("the user granted the invoke", &user_granted);
}

/// The caller's own policies and the api's document are read TOGETHER: an allow from either
/// serves the call, and an explicit deny from either refuses it.
#[tokio::test]
async fn either_grant_suffices_and_either_deny_refuses() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "identity-policy-with-document").await;
    given_session(&gateway);
    let method = deployed.method_arn(RESOURCE_PATH);
    let scoped = format!("execute-api:/{STAGE}/GET/{RESOURCE_PATH}");

    // The api's document allows the invoke while the role carries nothing: served, because
    // either grant is enough.
    given_policy(&gateway, Entity::Role, None);
    deployed
        .set_policy(
            &gateway,
            "the document allows it",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": INVOKE_ACTION,
                    "Resource": scoped,
                }]
            }),
        )
        .await;
    let served = invoke_as(&gateway, &deployed.api, SESSION_KEY_ID, RESOURCE_PATH).await;
    assert_served("the document allows it, the role grants nothing", &served);

    // The document DENIES it while the role grants it: refused, because an explicit deny on
    // either side wins over every grant.
    given_policy(&gateway, Entity::Role, Some(&allow(INVOKE_ACTION, &method)));
    deployed
        .set_policy(
            &gateway,
            "the document denies it",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": INVOKE_ACTION,
                    "Resource": "execute-api:/*",
                }]
            }),
        )
        .await;
    let refused = invoke_as(&gateway, &deployed.api, SESSION_KEY_ID, RESOURCE_PATH).await;
    assert_refused(
        "the document denies it, the role grants it",
        &refused,
        &session_arn(),
        &method,
    );
}

/// A method whose `authorizationType` authenticates NOBODY reads no identity policy at all:
/// API Gateway resolves no principal for it, so the same under-privileged credential that
/// is refused the `AWS_IAM` method reaches the public one.
#[tokio::test]
async fn a_method_that_authenticates_nobody_reads_no_identity_policy() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "identity-policy-open-method").await;
    given_session(&gateway);
    given_policy(&gateway, Entity::Role, None);
    let open = deployed.mock_method(&gateway, "open", "NONE").await;
    // Rows differ along the method's `authorizationType`, which is what decides whether the
    // request speaks for a principal at all.
    let rows: &[(&str, &str, bool)] = &[
        ("the AWS_IAM method", RESOURCE_PATH, false),
        ("the method that authenticates nobody", open.as_str(), true),
    ];
    for (case, path_part, served) in rows {
        let answered = invoke_as(&gateway, &deployed.api, SESSION_KEY_ID, path_part).await;
        if *served {
            assert_served(case, &answered);
        } else {
            assert_refused(case, &answered, &session_arn(), &deployed.method_arn(path_part));
        }
    }
}
