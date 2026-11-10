// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what is
//! asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation of this service evaluates the identity-based IAM
//! policies in force on the principal its credentials name — against
//! `license-manager-user-subscriptions:<OperationName>` on this service's ARN for the PATH
//! the request addressed — BEFORE the request is deserialized, constraint-checked or
//! answered on this service's own terms. A request no statement allows (IAM's default
//! deny), or one an explicit `Deny` covers, is answered `AccessDeniedException` at the
//! status this service's model gives that error, performs nothing and leaves no resource
//! behind; a request a matching `Allow` covers reaches the operation's own answer.
//!
//! The user, its access key and its policies live in the IAM store of the SAME shared
//! engine — the store IAM's own handlers write — so the documents under test are the ones
//! `iam:PutUserPolicy` stores and the principal a refusal blames is the one the account
//! holds that access key under.
//!
//! The tables span what the rule generalises over: the operation (all seventeen of them,
//! across all four HTTP methods its routes use), the document in force (none at all,
//! another action of this service, another service's actions, an explicit deny beside an
//! allow, an allow of exactly the action, of the action in another case, of the whole
//! service and of everything), the resource an allow is scoped to (every resource, one
//! plane's paths, one operation's path, a path in another Region) and the principal (an IAM
//! user holding a long-term key, and the account's own published credential).
//!
//! The harness (router + engine + the account's directories and network) is the one in
//! [`super::identity_provider_subscriptions`]; only the rules are new here.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

// ONE wire harness per service: the dispatch plumbing and the fixtures come from the
// sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    arn_prefix, encoded_label, harness, provider_union, Harness, Reply, ACCOUNT, PRODUCTS, REGION,
    SIGNING_NAME,
};

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`) and also its signing name. Spelled out here
/// rather than read from the production code, so a change to it fails a test.
const ACTION_PREFIX: &str = SIGNING_NAME;

/// The wire error code and the status an authorization refusal of this service carries:
/// `AccessDeniedException` is a shape of its model whose `errorKind` is client with no
/// `@httpError` (`smithy_query(query="shape",shape="AccessDeniedException")`), which
/// defaults to 400.
const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTESTEXAMPLE";

/// The access key id of the IAM user every judged row signs with, and that user's name.
const USER_KEY_ID: &str = "AKIAEXAMPLEUNPRIVILEG";
const USER_NAME: &str = "subscriptions-operator";

/// The name of the inline policy the user carries.
const POLICY_NAME: &str = "inline";

/// A directory of the account's own fleet, so a request that gets past authorization is
/// answered on this service's own terms rather than refused for a directory it cannot
/// resolve.
const DIRECTORY: &str = "d-0123456789";

/// A subnet of the account's own Amazon EC2 store, which a registration's `Settings` name.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The ARN the account holds that IAM user under, which a refusal names as the caller.
fn user_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{USER_NAME}")
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLESUBSCRIPTIONS".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Attach `document` to that user as its inline policy, replacing whatever it carried —
/// what `iam:PutUserPolicy` does. `None` takes every policy off it, as
/// `iam:DeleteUserPolicy` does.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store.user_inline_policies.insert(
            iam::name_key(USER_NAME),
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

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with. Spelled out here rather than read from the
    /// shared helper the gate words it through, so a change to either fails a test.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One operation of this service, as a client addresses it: the request line the rule is
/// judged on, and a body that makes it a request the operation would otherwise answer on
/// its own terms.
struct Operation {
    name: &'static str,
    method: &'static str,
    /// The URI PATH, which is what the resource the request is judged against is built
    /// from — so it is held apart from the query string.
    path: String,
    query: &'static str,
    body: Option<Value>,
}

/// Every operation this service serves, each with a request of its own. The rule covers all
/// of them, so the table is the whole surface rather than a sample of it — including the
/// three tagging operations, whose own model does not list `AccessDeniedException` but which
/// the IAM layer in front of the service decides exactly like the rest.
fn operations() -> Vec<Operation> {
    let tag_path = format!(
        "/tags/{}",
        encoded_label(&format!("{}ip-000000009999", arn_prefix("identity-provider")))
    );
    let provider_arn = format!("{}ip-000000009999", arn_prefix("identity-provider"));
    vec![
        Operation {
            name: "AssociateUser",
            method: "POST",
            path: "/user/AssociateUser".to_string(),
            query: "",
            body: Some(json!({
                "Username": "someone",
                "InstanceId": "i-0123456789abcdef0",
                "IdentityProvider": provider_union(DIRECTORY),
            })),
        },
        Operation {
            name: "CreateLicenseServerEndpoint",
            method: "POST",
            path: "/license-server/CreateLicenseServerEndpoint".to_string(),
            query: "",
            body: Some(json!({
                "IdentityProviderArn": provider_arn,
                "LicenseServerSettings": {
                    "ServerType": "RDS_SAL",
                    "ServerSettings": {
                        "RdsSalSettings": {
                            "RdsSalCredentialsProvider": {
                                "SecretsManagerCredentialsProvider": { "SecretId": "rds-sal" },
                            },
                        },
                    },
                },
            })),
        },
        Operation {
            name: "DeleteLicenseServerEndpoint",
            method: "POST",
            path: "/license-server/DeleteLicenseServerEndpoint".to_string(),
            query: "",
            body: Some(json!({
                "LicenseServerEndpointArn":
                    format!("{}lse-000000009999", arn_prefix("license-server-endpoint")),
                "ServerType": "RDS_SAL",
            })),
        },
        Operation {
            name: "DeregisterIdentityProvider",
            method: "POST",
            path: "/identity-provider/DeregisterIdentityProvider".to_string(),
            query: "",
            body: Some(json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
            })),
        },
        Operation {
            name: "DisassociateUser",
            method: "POST",
            path: "/user/DisassociateUser".to_string(),
            query: "",
            body: Some(json!({
                "InstanceUserArn": format!("{}iu-000000009999", arn_prefix("instance-user")),
            })),
        },
        Operation {
            name: "ListIdentityProviders",
            method: "POST",
            path: "/identity-provider/ListIdentityProviders".to_string(),
            query: "",
            body: Some(json!({})),
        },
        Operation {
            name: "ListInstances",
            method: "POST",
            path: "/instance/ListInstances".to_string(),
            query: "",
            body: Some(json!({})),
        },
        Operation {
            name: "ListLicenseServerEndpoints",
            method: "POST",
            path: "/license-server/ListLicenseServerEndpoints".to_string(),
            query: "",
            body: Some(json!({})),
        },
        Operation {
            name: "ListProductSubscriptions",
            method: "POST",
            path: "/user/ListProductSubscriptions".to_string(),
            query: "",
            body: Some(json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
            })),
        },
        Operation {
            name: "ListTagsForResource",
            method: "GET",
            path: tag_path.clone(),
            query: "",
            body: None,
        },
        Operation {
            name: "ListUserAssociations",
            method: "POST",
            path: "/user/ListUserAssociations".to_string(),
            query: "",
            body: Some(json!({
                "InstanceId": "i-0123456789abcdef0",
                "IdentityProvider": provider_union(DIRECTORY),
            })),
        },
        Operation {
            name: "RegisterIdentityProvider",
            method: "POST",
            path: "/identity-provider/RegisterIdentityProvider".to_string(),
            query: "",
            body: Some(json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
                "Settings": { "Subnets": [SUBNET], "SecurityGroupId": "sg-0123abcd" },
            })),
        },
        Operation {
            name: "StartProductSubscription",
            method: "POST",
            path: "/user/StartProductSubscription".to_string(),
            query: "",
            body: Some(json!({
                "Username": "someone",
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
            })),
        },
        Operation {
            name: "StopProductSubscription",
            method: "POST",
            path: "/user/StopProductSubscription".to_string(),
            query: "",
            body: Some(json!({
                "ProductUserArn": format!("{}pu-000000009999", arn_prefix("user")),
            })),
        },
        Operation {
            name: "TagResource",
            method: "PUT",
            path: tag_path.clone(),
            query: "",
            body: Some(json!({ "Tags": { "team": "licensing" } })),
        },
        Operation {
            name: "UntagResource",
            method: "DELETE",
            path: tag_path,
            query: "?tagKeys=team",
            body: None,
        },
        Operation {
            name: "UpdateIdentityProviderSettings",
            method: "POST",
            path: "/identity-provider/UpdateIdentityProviderSettings".to_string(),
            query: "",
            body: Some(json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
                "UpdateSettings": { "AddSubnets": [SUBNET], "RemoveSubnets": [] },
            })),
        },
    ]
}

/// Dispatch one operation signed with `access_key_id` — what names the principal the
/// request speaks for.
async fn call_as(h: &Harness, access_key_id: &str, operation: &Operation) -> Reply {
    let payload = operation
        .body
        .as_ref()
        .map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let request = http::Request::builder()
        .method(operation.method)
        .uri(format!(
            "https://{SIGNING_NAME}.{REGION}.amazonaws.com{}{}",
            operation.path, operation.query
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type = error_type.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_type,
        body,
    }
}

/// The resource ARN a request addressing `path` is judged against: this service's ARN with
/// that path as its resource field, in the Region the request was signed for.
fn resource_of(path: &str) -> String {
    format!("arn:aws:{ACTION_PREFIX}:{REGION}:{ACCOUNT}:{path}")
}

/// The documents a row puts in force on the user, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("license-manager:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["license-manager:GetLicense", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// THE RULE: every operation is judged against its OWN
/// `license-manager-user-subscriptions:<OperationName>` action before it performs anything;
/// a refusal is `AccessDeniedException` at the status the model gives it, naming the
/// principal, the action, the resource the request addressed and which of IAM's rules
/// refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for operation in operations() {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        // Another action of this service, which a row grants instead of the one under test.
        let other = operations()
            .into_iter()
            .map(|row| format!("{ACTION_PREFIX}:{}", row.name))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.name);
            // A fresh store, user and key per row: no row can pass on another's write.
            let h = harness();
            given_user_key(&h.engine);
            given_user_policy(&h.engine, document.as_ref());

            let answer = call_as(&h, USER_KEY_ID, &operation).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        (answer.status, answer.error_type.as_deref()),
                        (ACCESS_DENIED.0, Some(ACCESS_DENIED.1)),
                        "case {case:?}: expected the authorization refusal, got {answer:?}"
                    );
                    assert_eq!(
                        answer.message(),
                        Some(
                            format!(
                                "User: {} is not authorized to perform: {action} on resource: \
                                 {} {}",
                                user_arn(),
                                resource_of(&operation.path),
                                refusal.reason(&action)
                            )
                            .as_str()
                        ),
                        "case {case:?}: expected IAM's wording, got {answer:?}"
                    );
                }
                None => {
                    // Authorization answers BEFORE anything is read, so an allowed call
                    // falls through to the operation's own verdict — here a refusal about
                    // the resource nothing created, or a success for an operation that
                    // needs none — and never to another authorization decision.
                    assert_ne!(
                        answer.error_type.as_deref(),
                        Some(ACCESS_DENIED.1),
                        "case {case:?}: an allowed request must not be refused, got {answer:?}"
                    );
                }
            }
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// user's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for operation in operations() {
        let h = harness();
        given_user_key(&h.engine);
        // The user carries nothing at all, which is what refuses it above.
        given_user_policy(&h.engine, None);

        let answer = call_as(&h, ACCOUNT_KEY_ID, &operation).await;
        assert_ne!(
            answer.error_type.as_deref(),
            Some(ACCESS_DENIED.1),
            "case {:?}: the account's own credential must not be refused, got {answer:?}",
            operation.name
        );
    }
}

/// RULE: the decision is taken before the handler runs, so a refused call PERFORMS NOTHING:
/// a `RegisterIdentityProvider` no policy allows leaves no registration behind, which the
/// account's own credential — which the gate does not bound — then reads back.
#[tokio::test]
async fn a_refused_registration_creates_nothing() {
    let h = harness();
    given_user_key(&h.engine);
    given_user_policy(&h.engine, None);

    let register = operations()
        .into_iter()
        .find(|row| row.name == "RegisterIdentityProvider")
        .expect("the registration operation");
    let refused = call_as(&h, USER_KEY_ID, &register).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (ACCESS_DENIED.0, Some(ACCESS_DENIED.1)),
        "the unauthorized registration must be refused, got {refused:?}"
    );
    assert!(
        refused.body.get("IdentityProviderSummary").is_none(),
        "a refused registration must report no provider, got {refused:?}"
    );

    let listing = operations()
        .into_iter()
        .find(|row| row.name == "ListIdentityProviders")
        .expect("the listing operation");
    let listed = call_as(&h, ACCOUNT_KEY_ID, &listing).await;
    assert_eq!(
        listed
            .ok("the account reads the registrations back")
            .get("IdentityProviderSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused registration must have created nothing, got {listed:?}"
    );
}

/// RULE: a request is judged against the ARN of the PATH it addressed, so an allow scoped
/// to one path covers the operations that address it and no other — including a path of the
/// same shape in another Region.
#[tokio::test]
async fn an_action_is_judged_against_the_path_the_request_addressed() {
    // The three operations this case drives, one per plane, and the paths an allow is
    // scoped to.
    let judged = ["ListInstances", "ListIdentityProviders", "ListInstances"];
    let instance_plane = resource_of("/instance/*");
    let one_operation = resource_of("/instance/ListInstances");
    let another_region = format!("arn:aws:{ACTION_PREFIX}:eu-central-1:{ACCOUNT}:/instance/*");

    // (case, the Resource the allow is scoped to, which operations it covers)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "every resource",
            "*".to_string(),
            vec!["ListInstances", "ListIdentityProviders"],
        ),
        (
            "the instance plane alone",
            instance_plane,
            vec!["ListInstances"],
        ),
        (
            "exactly one operation's path",
            one_operation,
            vec!["ListInstances"],
        ),
        ("the instance plane in another Region", another_region, Vec::new()),
    ];

    for (case, resource, covered) in rows {
        for operation in operations() {
            if !judged.contains(&operation.name) {
                continue;
            }
            let h = harness();
            given_user_key(&h.engine);
            given_user_policy(
                &h.engine,
                Some(&json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": format!("{ACTION_PREFIX}:*"),
                        "Resource": resource.clone(),
                    }],
                })),
            );

            let answer = call_as(&h, USER_KEY_ID, &operation).await;
            let allowed = covered.contains(&operation.name);
            assert_eq!(
                answer.error_type.as_deref() == Some(ACCESS_DENIED.1),
                !allowed,
                "case {case:?}/{}: an allow scoped to {resource:?} must {}cover this \
                 operation, got {answer:?}",
                operation.name,
                if allowed { "" } else { "not " }
            );
        }
    }
}
