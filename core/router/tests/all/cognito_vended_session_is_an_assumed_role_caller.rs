// SPDX-License-Identifier: CC0-1.0

//! WHO the credentials an identity pool vends speak for, across the services a
//! Cognito-federated caller spans.
//!
//! RULE: `GetCredentialsForIdentity` answers the credentials of an
//! `sts:AssumeRoleWithWebIdentity` session of the role the pool resolves for THAT request,
//! so a request signed with them resolves to that ROLE SESSION:
//! `sts:GetCallerIdentity` answers
//! `arn:<partition>:sts::<account>:assumed-role/<role-name>/CognitoIdentityCredentials`,
//! with `UserId` `<RoleId>:CognitoIdentityCredentials` for the id IAM issued that role and
//! `Account` the role's own account — never the long-term principal of an IAM user. A
//! federated caller, and an API Gateway integration behind it, reads which role was vended
//! off that ARN, which it can do only while the assumed-role segment is there.
//!
//! Only a cross-service suite can pin this: the user pool whose login makes the exchange an
//! authenticated one is created by `cognito-idp`, the role is written by `iam:CreateRole`,
//! the session is minted by the vend, and it is resolved by `sts:GetCallerIdentity` — four
//! services over ONE engine, as `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire. The vend legs carry no credential scope at all (their
//! model lists no auth scheme); the setup legs and the caller-identity legs carry one, on a
//! router with request authentication off as every suite in this crate runs it.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the pools and the
/// identities live in.
const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header names no service and the
/// unsigned vend legs carry their service name in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id every signed setup call carries — a long-term key, which is what makes
/// it the contrast case: it resolves to a user principal, not to a role session.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The `RoleSessionName` every session an identity pool vends carries, written here as AWS
/// publishes it in the caller ARN of such a session
/// (`arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`) rather than
/// read back from the implementation, so a session name that drifted from it would fail.
const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The STS API version every `awsQuery` call to STS carries.
const STS_API_VERSION: &str = "2011-06-15";

/// The IAM API version every `awsQuery` call to IAM carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// What one call answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request, signed with `access_key_id` for `signing_name`, or unsigned when
/// `signing_name` is `None` — which is how an SDK sends an operation whose model lists no
/// auth scheme.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing_name {
        Some((name, _)) => format!("{name}.{REGION}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone());
    if let Some((signing_name, access_key_id)) = signing_name {
        // The signature is never checked: this router runs with authentication off. What
        // the scope carries is the key id and the Region a handler reads.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsJson1_1 call to `target`, signed for `signing_name` with [`SETUP_KEY_ID`] or
/// unsigned.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing_name.map(|name| (name, SETUP_KEY_ID)),
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// One awsQuery call, which is how IAM and STS are called, signed with `access_key_id`.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    access_key_id: &str,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        Some((signing_name, access_key_id)),
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// A role trust policy admitting the Cognito Identity principal for the identity pool
/// `audience`, written the way the Amazon Cognito Developer Guide writes one: the
/// `Federated` principal, the web-identity assume, and the `:aud` condition that restricts
/// the assume to users of that one pool.
///
/// Compact and free of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn trust_policy(audience: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{audience}\"}}}}}}]}}"
    )
}

/// One role as `iam:CreateRole` wrote it: its ARN, and the `RoleId` IAM issued it — the id
/// the session's `UserId` has to be built from.
struct SeededRole {
    arn: String,
    role_id: String,
}

/// One row of the table below: a vend request the pool resolves a role for, and the role it
/// has to resolve.
struct Row<'a> {
    /// What this row is, named in every assertion it drives.
    case: &'a str,
    /// The `Logins` the request presents, which is what makes it an authenticated exchange.
    logins: Option<&'a Value>,
    /// The `CustomRoleArn` the request names, if any.
    custom_role_arn: Option<&'a str>,
    /// The name of the role the vended session must stand for.
    role_name: &'a str,
    /// That role, as IAM wrote it.
    role: &'a SeededRole,
}

/// The identity token an Amazon Cognito user pool issues, as the `Logins` entry made under
/// its provider name carries it: a compact JWS whose header names a signing algorithm, whose
/// claim set names that provider as the issuer (`iss`) and a user as the subject, and whose
/// signature segment is filled (RFC 7515 §3.1, RFC 7519 §4.1). Nothing in the emulator
/// verifies the signature — that needs the issuer's published key — so what a presented
/// login is decided by is this form and these claims.
fn id_token_of(provider_name: &str) -> String {
    let segment = |value: &str| {
        aws_smithy_types::base64::encode(value.as_bytes())
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_")
    };
    format!(
        "{}.{}.{}",
        segment(r#"{"alg":"RS256","kid":"kid-1","typ":"JWT"}"#),
        segment(
            &json!({
                "iss": format!("https://{provider_name}"),
                "sub": "a-user-of-the-pool",
                "token_use": "id",
                "exp": 4_102_444_800u64,
            })
            .to_string()
        ),
        segment("a-signature-nothing-here-verifies")
    )
}

/// `iam:CreateRole` with a trust policy admitting `audience`'s users.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    audience: &str,
) -> SeededRole {
    let document = trust_policy(audience);
    let created = query(
        router,
        runtime,
        "iam",
        SETUP_KEY_ID,
        format!(
            "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={role_name}\
             &AssumeRolePolicyDocument={document}"
        ),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let text = created.text();
    SeededRole {
        arn: xml_element(&text, "Arn"),
        role_id: xml_element(&text, "RoleId"),
    }
}

/// `cognito-idp:CreateUserPool`, answering the user pool's id.
fn create_user_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
) -> String {
    let created = rpc(
        router,
        runtime,
        Some("cognito-idp"),
        "AWSCognitoIdentityProviderService.CreateUserPool",
        &json!({"PoolName": pool_name}),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateUserPool({pool_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool reports the pool's Id, got {}", created.text()))
        .to_string()
}

/// `cognito-idp:CreateUserPoolClient`, answering the app client's id.
fn create_user_pool_client(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = rpc(
        router,
        runtime,
        Some("cognito-idp"),
        "AWSCognitoIdentityProviderService.CreateUserPoolClient",
        &json!({"UserPoolId": user_pool_id, "ClientName": client_name}),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateUserPoolClient({client_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "CreateUserPoolClient reports the client's ClientId, got {}",
                created.text()
            )
        })
        .to_string()
}

/// `cognito-identity:CreateIdentityPool` for a pool that accepts both flows: unauthenticated
/// identities, and logins of the user pool `provider_name` names.
fn create_identity_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
    provider_name: &str,
    client_id: &str,
) -> String {
    let created = rpc(
        router,
        runtime,
        Some("cognito-identity"),
        "AWSCognitoIdentityService.CreateIdentityPool",
        &json!({
            "IdentityPoolName": pool_name,
            "AllowUnauthenticatedIdentities": true,
            "CognitoIdentityProviders": [
                {"ProviderName": provider_name, "ClientId": client_id},
            ],
        }),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateIdentityPool({pool_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "CreateIdentityPool reports an IdentityPoolId, got {}",
                created.text()
            )
        })
        .to_string()
}

/// `cognito-identity:SetIdentityPoolRoles`: the pool's two roles, plus a mapping rule over
/// `provider_name` naming a third — the set of roles a `CustomRoleArn` can be drawn from.
fn set_identity_pool_roles(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    authenticated: &str,
    unauthenticated: &str,
    provider_name: &str,
    mapped: &str,
) {
    let configured = rpc(
        router,
        runtime,
        Some("cognito-identity"),
        "AWSCognitoIdentityService.SetIdentityPoolRoles",
        &json!({
            "IdentityPoolId": pool_id,
            "Roles": {"authenticated": authenticated, "unauthenticated": unauthenticated},
            "RoleMappings": {
                provider_name: {
                    "Type": "Rules",
                    // A token whose claims match no rule falls to the pool's own
                    // authenticated role, which is the role the flow resolves for an
                    // authenticated exchange — so what the rows below vary is the flow and
                    // the `CustomRoleArn`, not whether a rule happened to match.
                    "AmbiguousRoleResolution": "AuthenticatedRole",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "custom:tier",
                        "MatchType": "Equals",
                        "Value": "gold",
                        "RoleARN": mapped,
                    }]},
                },
            },
        }),
    );
    assert_eq!(
        configured.status,
        200,
        "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
        configured.status,
        configured.text()
    );
}

/// An unsigned `GetId`, answering the identity it minted.
fn get_id(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    logins: Option<&Value>,
) -> String {
    let mut request = json!({"IdentityPoolId": pool_id});
    if let Some(logins) = logins {
        request["Logins"] = logins.clone();
    }
    let minted = rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetId",
        &request,
    );
    assert_eq!(
        minted.status,
        200,
        "an unsigned GetId must be served, got {} with {:?}",
        minted.status,
        minted.text()
    );
    minted.json()["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId reports an IdentityId, got {}", minted.text()))
        .to_string()
}

/// An unsigned `GetCredentialsForIdentity`.
fn vend(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    identity_id: &str,
    logins: Option<&Value>,
    custom_role_arn: Option<&str>,
) -> Answer {
    let mut request = json!({"IdentityId": identity_id});
    if let Some(logins) = logins {
        request["Logins"] = logins.clone();
    }
    if let Some(custom_role_arn) = custom_role_arn {
        request["CustomRoleArn"] = json!(custom_role_arn);
    }
    rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &request,
    )
}

/// `sts:GetCallerIdentity` under `access_key_id`.
fn caller_identity(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    query(
        router,
        runtime,
        "sts",
        access_key_id,
        format!("Action=GetCallerIdentity&Version={STS_API_VERSION}"),
    )
}

/// The role name a federated caller reads off a caller ARN, the way such a caller reads it:
/// the segment between `:assumed-role/` and the role session name. `None` when the ARN
/// carries no assumed-role segment at all, which is the shape no federated caller can
/// identify its session from.
fn assumed_role_name(arn: &str) -> Option<&str> {
    arn.split_once(":assumed-role/")
        .map(|(_, session)| session.split_once('/').map_or(session, |(role, _)| role))
}

#[test]
fn a_vended_session_resolves_to_its_assumed_role_caller() {
    std::thread::Builder::new()
        .name("cognito-vended-session-caller".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The user pool whose logins make an exchange an authenticated one, named the
            // way `Logins` names one.
            let user_pool_id = create_user_pool(&router, &runtime, "federated-callers");
            let client_id =
                create_user_pool_client(&router, &runtime, &user_pool_id, "federated-app");
            let provider_name = format!("cognito-idp.{REGION}.amazonaws.com/{user_pool_id}");
            let pool_id = create_identity_pool(
                &router,
                &runtime,
                "federated-callers",
                &provider_name,
                &client_id,
            );

            // Three roles the pool can vend a session of, their names differing in form —
            // IAM's `RoleName` admits word characters, `.`, `-`, `@`, `=`, `,` and `+` —
            // so a caller parsing the role out of its ARN is read on more than one shape.
            let unauthenticated = create_role(&router, &runtime, "fed-unauth", &pool_id);
            let authenticated = create_role(&router, &runtime, "Fed_Auth.Role-2", &pool_id);
            let mapped = create_role(&router, &runtime, "fed.mapped@role-3", &pool_id);
            set_identity_pool_roles(
                &router,
                &runtime,
                &pool_id,
                &authenticated.arn,
                &unauthenticated.arn,
                &provider_name,
                &mapped.arn,
            );

            // The `Logins` entry the authenticated rows present: the identity token that
            // user pool issues, under the user pool's own provider name — a compact JWS
            // naming that provider as its issuer, which is what a login of a user pool is.
            let provider = provider_name.clone();
            let login = json!({provider: id_token_of(&provider_name)});
            // The rows differ along what the rule generalises over: which role the pool
            // resolves for the request — the flow it is in, and whether it names a
            // `CustomRoleArn` among the roles the pool holds — and the form of that role's
            // name.
            let rows = vec![
                Row {
                    case: "no login: a session of the pool's unauthenticated role",
                    logins: None,
                    custom_role_arn: None,
                    role_name: "fed-unauth",
                    role: &unauthenticated,
                },
                Row {
                    case: "a user-pool login: a session of the pool's authenticated role",
                    logins: Some(&login),
                    custom_role_arn: None,
                    role_name: "Fed_Auth.Role-2",
                    role: &authenticated,
                },
                Row {
                    case: "a CustomRoleArn a mapping rule names: a session of THAT role",
                    logins: Some(&login),
                    custom_role_arn: Some(mapped.arn.as_str()),
                    role_name: "fed.mapped@role-3",
                    role: &mapped,
                },
            ];

            for Row {
                case,
                logins,
                custom_role_arn,
                role_name,
                role,
            } in rows
            {
                let identity_id = get_id(&router, &runtime, &pool_id, logins);
                let vended = vend(&router, &runtime, &identity_id, logins, custom_role_arn);
                assert_eq!(
                    vended.status,
                    200,
                    "case {case:?}: the vend must be served, got {} with {:?}",
                    vended.status,
                    vended.text()
                );
                let access_key_id = vended.json()["Credentials"]["AccessKeyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !access_key_id.is_empty(),
                    "case {case:?}: the vend must carry an AccessKeyId, got {:?}",
                    vended.text()
                );

                let answered = caller_identity(&router, &runtime, &access_key_id);
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: GetCallerIdentity under the vended credential must \
                     answer 200, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                let body = answered.text();
                let arn = xml_element(&body, "Arn");
                let expected = format!(
                    "arn:aws:sts::{}:assumed-role/{role_name}/{VENDED_ROLE_SESSION_NAME}",
                    engine::emulator_account_id()
                );
                assert_eq!(
                    arn, expected,
                    "case {case:?}: expected the caller ARN {expected}, got {arn}"
                );
                // What a federated caller does with that ARN: read the role off it.
                assert_eq!(
                    assumed_role_name(&arn),
                    Some(role_name),
                    "case {case:?}: a caller reading the vended role off the ARN expected \
                     {role_name}, got {arn}"
                );
                let user_id = xml_element(&body, "UserId");
                assert_eq!(
                    user_id,
                    format!("{}:{VENDED_ROLE_SESSION_NAME}", role.role_id),
                    "case {case:?}: the session's UserId is the role's own id and the \
                     session name, got {user_id}"
                );
                assert_eq!(
                    xml_element(&body, "Account"),
                    engine::emulator_account_id(),
                    "case {case:?}: the session belongs to the role's account, got {body}"
                );
            }

            // The contrast that makes the rule a rule about the VEND: a long-term key of
            // the same account resolves to a user principal, which carries no assumed-role
            // segment, so an ARN that does is one the vend produced.
            let ambient = caller_identity(&router, &runtime, SETUP_KEY_ID);
            assert_eq!(
                ambient.status,
                200,
                "GetCallerIdentity under a long-term key must answer 200, got {} with {:?}",
                ambient.status,
                ambient.text()
            );
            let ambient_arn = xml_element(&ambient.text(), "Arn");
            assert_eq!(
                assumed_role_name(&ambient_arn),
                None,
                "a long-term key names no role session, got {ambient_arn}"
            );
        })
        .expect("spawn the vended-session caller thread")
        .join()
        .expect("a vended session must resolve to its assumed-role caller");
}
