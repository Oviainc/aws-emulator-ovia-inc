// SPDX-License-Identifier: CC0-1.0

//! Federating an OpenID Connect ID token into an identity pool through the trust the pool
//! states for the token's ISSUER, over the same `MultiServiceRouter` the production router
//! builds, with IAM mounted on the same engine so the OIDC providers are the ones
//! `iam:CreateOpenIDConnectProvider` wrote.
//!
//! Two rules are under test.
//!
//! * ADMISSION: an identity pool admits a `Logins` entry only where its stored
//!   configuration states a trust for the entry's issuer — a `CognitoIdentityProviders`
//!   entry naming it, or an `OpenIdConnectProviderARNs` entry, whose ARN names the issuer
//!   and whose IAM registration holds the client ids ("audiences") that trust accepts —
//!   and refuses it `NotAuthorizedException` (HTTP 400) when the presented token names an
//!   audience that trust does not accept. A trust whose accepted audiences this emulator
//!   cannot know constrains none: another account's provider ARN, and a user pool named by
//!   `CognitoIdentityProviders`, whose own token validation is not this rule. An issuer the
//!   pool states no trust for is refused where the issuer is one this emulator itself hosts
//!   (an Amazon Cognito user pool, which its provider name names); any other issuer's token
//!   is decided by that issuer, which this emulator does not reach.
//! * ONE IDENTITY PER FEDERATED USER: `Logins` maps a provider name to a
//!   "provider_user_identifier", so an admitted login resolves to the identity of the user
//!   its token NAMES — every token naming one subject, whatever app client it was issued
//!   for, is that one user.
//!
//! The rows differ along what the rules generalise over: which stored member states the
//! trust, the shape of the issuer (a user pool's, and an unrelated OIDC host with a path),
//! how the token spells its audience (one string, a list), whether the audience is one the
//! trust accepts, which subject the token names, and — across the suites — what the
//! configuration does next (another client id registered with the provider, the provider
//! deleted, the ARN dropped from the pool).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::base64_url;

use crate::developer_credentials_authorization::{form_encode, xml_element};
use crate::identity_pool_flow::{cognito_trust_policy, error_code, seed_role, seeded_role_arn};

/// The Region every call below is signed for, and the one the issuers of the user pools in
/// these fixtures name.
const REGION: &str = "us-east-1";

/// The access-key id the IAM setup calls carry: no IAM or STS record stands for it, so it
/// resolves to the account's ambient identity.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// Three app clients, told apart by nothing but their value — "there is no defined format
/// for a client ID" (`iam:CreateOpenIDConnectProvider$ClientIDList`).
pub(crate) const CLIENT_A: &str = "1h57kf5cpq17m0eml12eqpq8m2";
pub(crate) const CLIENT_B: &str = "4bv7g2kq9n3l1pd8s0a6c5e7f1";
pub(crate) const CLIENT_C: &str = "7qz0x3m8t2r5y9w1v4n6b8k3j5";

/// A 40-character hex thumbprint, the length `iam:CreateOpenIDConnectProvider`'s
/// `thumbprintType` declares.
const THUMBPRINT: &str = "0000000000000000000000000000000000000001";

/// The `Logins` provider name of an Amazon Cognito user pool: "Amazon Cognito user pool:
/// `cognito-idp.<region>.amazonaws.com/<YOUR_USER_POOL_ID>`" (`GetId.Logins` in this
/// service's model), which is also the `iss` claim of that pool's ID tokens without its
/// scheme.
pub(crate) fn user_pool_issuer(user_pool_id: &str) -> String {
    format!("cognito-idp.{REGION}.amazonaws.com/{user_pool_id}")
}

/// An emulator holding this service beside IAM on ONE shared engine — the OIDC providers a
/// pool's ARNs resolve against are IAM's own records, so they are written through IAM's own
/// operations.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here is really signed: the `Authorization` header carries only the
        // credential scope, which is what routes a request and names its Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One `awsJson1_1` call to this service, with its status and parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     cognito-identity/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&payload)
                )
            })
        };
        (status, parsed)
    }

    /// A call to this service that must succeed, with its output object.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "setup {operation} on {body} expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One `awsQuery` call to IAM, with its status and XML body.
    async fn iam(&self, action: &str, params: &[(String, String)]) -> (u16, String) {
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
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }

    /// An IAM call that must succeed, with its XML body.
    async fn iam_ok(&self, action: &str, params: &[(String, String)]) -> String {
        let (status, body) = self.iam(action, params).await;
        assert_eq!(
            status, 200,
            "setup iam:{action} expected HTTP 200, got {status} with {body}"
        );
        body
    }

    /// Register an IAM OIDC provider for `issuer` accepting `client_ids`, and answer the
    /// ARN IAM keyed it under — the handle an identity pool names it by.
    pub(crate) async fn oidc_provider(&self, issuer: &str, client_ids: &[&str]) -> String {
        let mut params = vec![
            ("Url".to_string(), format!("https://{issuer}")),
            ("ThumbprintList.member.1".to_string(), THUMBPRINT.to_string()),
        ];
        for (index, client_id) in client_ids.iter().enumerate() {
            params.push((
                format!("ClientIDList.member.{}", index + 1),
                (*client_id).to_string(),
            ));
        }
        let created = self
            .iam_ok("CreateOpenIDConnectProvider", &params)
            .await;
        xml_element(&created, "OpenIDConnectProviderArn").unwrap_or_else(|| {
            panic!("CreateOpenIDConnectProvider answers the provider's ARN, got {created}")
        })
    }

    /// Create a pool with `configuration` merged over the members every fixture pool
    /// carries, give it an authenticated role that trusts its own users, and answer its
    /// minted `IdentityPoolId`.
    pub(crate) async fn pool(&self, name: &str, configuration: Value) -> String {
        let mut request = json!({
            "IdentityPoolName": name,
            "AllowUnauthenticatedIdentities": true,
        });
        let members = request.as_object_mut().expect("a request object");
        for (member, value) in configuration.as_object().expect("a configuration object") {
            members.insert(member.clone(), value.clone());
        }
        let created = self.ok("CreateIdentityPool", &request).await;
        let pool_id = created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
            .to_string();
        let role_arn = seed_role(
            &self.engine,
            &seeded_role_arn(name, "auth"),
            cognito_trust_policy(&pool_id),
        );
        self.ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
            }),
        )
        .await;
        pool_id
    }
}

// The awsQuery helpers this suite drives IAM with — percent-encoding one form parameter
// and reading the text of one XML element — are this target's own
// (`developer_credentials_authorization`), so a setup call is built and read the one way
// across the suites; nothing is re-derived here.

/// The subject every fixture token below names unless a row varies it: the identifier of
/// the user the issuer authenticated.
const SUBJECT: &str = "7f3a1c2e-0000-4000-8000-0123456789ab";

/// An OpenID Connect ID token of `issuer`, issued for `audience`, naming [`SUBJECT`].
pub(crate) fn id_token(issuer: &str, audience: Value) -> String {
    id_token_for(issuer, audience, SUBJECT, "jti-1")
}

/// An OpenID Connect ID token of `issuer`, issued for `audience`, naming `subject`, with
/// `token_id` as its `jti` so two tokens of one user are two distinct values.
///
/// A compact JWS whose claims are the registered ones an ID token carries (RFC 7519 §4.1):
/// the issuer as a URL, the audience the token was issued for — a single string or a list,
/// both of which §4.1.3 allows — the subject, and a window that is still open. The
/// signature segment is filled, as an issuer's token carries one; nothing here verifies it,
/// and the emulator must not grant anything on it.
fn id_token_for(issuer: &str, audience: Value, subject: &str, token_id: &str) -> String {
    let issued_at = ::std::time::SystemTime::now()
        .duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let header = json!({"alg": "RS256", "kid": "fixture-key-1", "typ": "JWT"});
    let claims = json!({
        "iss": format!("https://{issuer}"),
        "aud": audience,
        "sub": subject,
        "token_use": "id",
        "jti": token_id,
        "iat": issued_at,
        "exp": issued_at + 3600,
    });
    format!(
        "{}.{}.{}",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes()),
        base64_url(b"a-signature-nothing-here-verifies")
    )
}

/// The wire code every refusal of a login this pool does not admit carries.
pub(crate) const NOT_AUTHORIZED: &str = "NotAuthorizedException";

/// The two sentences AWS words those refusals with, spelled out here so the wire-visible
/// text is pinned by the test rather than read back out of the crate.
const UNSUPPORTED_PROVIDER_MESSAGE: &str =
    "Token is not from a supported provider of this identity pool.";
pub(crate) const INCORRECT_AUDIENCE_MESSAGE: &str =
    "Invalid login token. Incorrect token audience.";
/// The sentence a refusal of a value that is no identity token at all carries, pinned here
/// for the same reason.
const NOT_AN_IDENTITY_TOKEN_MESSAGE: &str =
    "Invalid login token. Not a valid OpenId Connect identity token.";

/// Present `logins` to `GetId` and assert the pool admits it, then exchange the identity it
/// answers for credentials with the same logins and assert the vend is served. Answers the
/// `IdentityId`.
pub(crate) async fn admitted(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    logins: &Value,
) -> String {
    let (status, body) = emulator
        .call(
            "GetId",
            &json!({"IdentityPoolId": pool_id, "Logins": logins}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an admitted login must reach an identity, got {status} with {body}"
    );
    let identity_id = body["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served GetId answers an IdentityId: {body}"))
        .to_string();

    // The identity GetId minted is one a later read resolves, carrying the provider the
    // login was presented under.
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": &identity_id}))
        .await;
    let linked: Vec<&str> = described["Logins"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: an identity carries Logins: {described}"))
        .iter()
        .map(|login| login.as_str().unwrap_or_default())
        .collect();
    for provider in logins.as_object().expect("a Logins map").keys() {
        assert!(
            linked.contains(&provider.as_str()),
            "case {case:?}: expected {identity_id} to be linked to {provider:?}, got {linked:?}"
        );
    }

    let (status, body) = emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id, "Logins": logins}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an admitted login must vend credentials, got {status} with {body}"
    );
    assert!(
        body["Credentials"]["AccessKeyId"]
            .as_str()
            .is_some_and(|key| !key.is_empty()),
        "case {case:?}: a served vend carries credentials, got {body}"
    );
    identity_id
}

/// Present `logins` to `GetId` and to `GetCredentialsForIdentity` for `identity_id`, and
/// assert both are refused 400 `NotAuthorizedException` with `message`.
pub(crate) async fn refused(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    identity_id: &str,
    logins: &Value,
    message: &str,
) {
    for (operation, body) in [
        (
            "GetId",
            json!({"IdentityPoolId": pool_id, "Logins": logins}),
        ),
        (
            "GetCredentialsForIdentity",
            json!({"IdentityId": identity_id, "Logins": logins}),
        ),
    ] {
        let (status, payload) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, NOT_AUTHORIZED),
            "case {case:?} on {operation}: expected (400, {NOT_AUTHORIZED}), got ({status}, \
             {payload})"
        );
        assert_eq!(
            payload["message"].as_str(),
            Some(message),
            "case {case:?} on {operation}: expected the message {message:?}, got {payload}"
        );
        assert_eq!(
            payload.get("Credentials"),
            None,
            "case {case:?} on {operation}: a refusal carries no Credentials, got {payload}"
        );
    }
}

/// One row of the admission table.
struct Row {
    case: &'static str,
    /// The issuer the login is presented under.
    issuer: String,
    /// The `aud` the presented token carries, or `None` for a token that is no JSON Web
    /// Token at all and so names no audience this emulator can read.
    audience: Option<Value>,
    /// The refusal the pool answers, or `None` when it admits the login.
    refusal: Option<&'static str>,
}

/// RULE: the pool's trust for the token's issuer, and the audience that trust accepts,
/// decide every presented login — whichever stored member states the trust.
#[tokio::test]
async fn a_login_is_admitted_by_the_pools_trust_for_its_issuer_and_the_audience_it_accepts() {
    let emulator = Emulator::new();

    // A user pool's issuer and an unrelated OIDC host with a path, each registered as an
    // IAM OIDC provider; a THIRD provider the account holds but the pool does not name.
    let federated_pool = user_pool_issuer("us-east-1_oidcpool1");
    let tenant_issuer = "auth.example.com/tenant-7".to_string();
    let unnamed_pool = user_pool_issuer("us-east-1_unnamed01");
    let federated_arn = emulator.oidc_provider(&federated_pool, &[CLIENT_A]).await;
    let tenant_arn = emulator
        .oidc_provider(&tenant_issuer, &[CLIENT_A, CLIENT_B])
        .await;
    emulator.oidc_provider(&unnamed_pool, &[CLIENT_A]).await;

    // The pool trusts those two issuers through IAM, and one more user pool directly
    // through `CognitoIdentityProviders` — the other member that states a trust — plus one
    // OIDC provider ARN of ANOTHER account, whose registration this emulator does not hold.
    let configured_pool = user_pool_issuer("us-east-1_cipool001");
    let foreign_issuer = "accounts.foreign.example".to_string();
    let foreign_arn = format!("arn:aws:iam::123456789012:oidc-provider/{foreign_issuer}");
    let pool_id = emulator
        .pool(
            "oidc-federation",
            json!({
                "OpenIdConnectProviderARNs": [&federated_arn, &tenant_arn, &foreign_arn],
                "CognitoIdentityProviders": [
                    {"ProviderName": &configured_pool, "ClientId": CLIENT_C},
                ],
            }),
        )
        .await;

    // An identity of this pool to exchange with, minted with no login at all.
    let identity_id = emulator
        .ok("GetId", &json!({"IdentityPoolId": &pool_id}))
        .await["IdentityId"]
        .as_str()
        .expect("GetId answers an IdentityId")
        .to_string();

    let rows: Vec<Row> = vec![
        Row {
            case: "a user-pool token, trusted through the pool's OIDC provider ARN",
            issuer: federated_pool.clone(),
            audience: Some(json!(CLIENT_A)),
            refusal: None,
        },
        Row {
            case: "the same issuer, issued for a client its provider does not accept",
            issuer: federated_pool.clone(),
            audience: Some(json!(CLIENT_B)),
            refusal: Some(INCORRECT_AUDIENCE_MESSAGE),
        },
        Row {
            case: "another OIDC issuer with a path, issued for one of its two clients",
            issuer: tenant_issuer.clone(),
            audience: Some(json!(CLIENT_B)),
            refusal: None,
        },
        Row {
            case: "the same issuer, issued for a client registered with the OTHER provider",
            issuer: tenant_issuer.clone(),
            audience: Some(json!(CLIENT_C)),
            refusal: Some(INCORRECT_AUDIENCE_MESSAGE),
        },
        Row {
            case: "an audience LIST, one entry of which the provider accepts",
            issuer: tenant_issuer.clone(),
            audience: Some(json!([CLIENT_C, CLIENT_A])),
            refusal: None,
        },
        Row {
            case: "an audience LIST, none of whose entries it accepts",
            issuer: tenant_issuer.clone(),
            audience: Some(json!([CLIENT_C, "another-client"])),
            refusal: Some(INCORRECT_AUDIENCE_MESSAGE),
        },
        Row {
            case: "a user-pool token, trusted through CognitoIdentityProviders",
            issuer: configured_pool.clone(),
            audience: Some(json!(CLIENT_C)),
            refusal: None,
        },
        Row {
            // A `CognitoIdentityProviders` entry states the issuer it trusts; validating the
            // user pool's own token against the user pool that minted it — its audience
            // among the rest — is that validation's own business, so this trust states no
            // audience constraint and the login is admitted.
            case: "the same issuer, issued for another client of that user pool",
            issuer: configured_pool.clone(),
            audience: Some(json!(CLIENT_A)),
            refusal: None,
        },
        Row {
            // Another account's provider: its registration is held somewhere this emulator
            // does not reach, so the audiences it accepts are not knowable here.
            case: "an ARN naming another account's provider, any audience",
            issuer: foreign_issuer.clone(),
            audience: Some(json!("a-client-of-another-account")),
            refusal: None,
        },
        Row {
            case: "a user pool the ACCOUNT registered but this pool does not name",
            issuer: unnamed_pool.clone(),
            audience: Some(json!(CLIENT_A)),
            refusal: Some(UNSUPPORTED_PROVIDER_MESSAGE),
        },
        Row {
            case: "a user pool nothing registered anywhere",
            issuer: user_pool_issuer("us-east-1_strange01"),
            audience: Some(json!(CLIENT_A)),
            refusal: Some(UNSUPPORTED_PROVIDER_MESSAGE),
        },
        Row {
            case: "a trusted issuer, presenting an identity token that names no audience",
            issuer: federated_pool.clone(),
            audience: None,
            refusal: None,
        },
    ];

    for row in &rows {
        let token = match &row.audience {
            Some(audience) => id_token(&row.issuer, audience.clone()),
            // An identity token carrying no `aud` claim names no audience to decide it on,
            // so the pool's trust for its issuer alone admits it.
            None => id_token(&row.issuer, Value::Null),
        };
        let logins = json!({ row.issuer.as_str(): token });
        match row.refusal {
            None => {
                admitted(&emulator, row.case, &pool_id, &logins).await;
            }
            Some(message) => {
                refused(
                    &emulator,
                    row.case,
                    &pool_id,
                    &identity_id,
                    &logins,
                    message,
                )
                .await;
            }
        }
    }

    // The ISSUER of these logins issues identity tokens, so a value that is no such token
    // at all is refused on the token itself, before the pool's trust for its issuer is
    // consulted — whatever that trust accepts.
    refused(
        &emulator,
        "a trusted issuer, presenting a value that is no identity token",
        &pool_id,
        &identity_id,
        &json!({ federated_pool.as_str(): "an-opaque-provider-token" }),
        NOT_AN_IDENTITY_TOKEN_MESSAGE,
    )
    .await;
}

/// RULE, as the IAM registration changes under a live pool: what the pool's provider ARN
/// resolves to at the instant the login is presented decides it, so registering another
/// client id with the provider admits a token the provider had not accepted, and deleting
/// the provider stops the pool admitting its issuer at all — while the pool's own stored
/// configuration, the dangling ARN included, is left exactly as it was.
#[tokio::test]
async fn what_the_pools_provider_arn_resolves_to_now_decides_the_login() {
    let emulator = Emulator::new();
    let issuer = user_pool_issuer("us-east-1_livepool1");
    let provider_arn = emulator.oidc_provider(&issuer, &[CLIENT_B]).await;
    let pool_id = emulator
        .pool(
            "oidc-registration",
            json!({"OpenIdConnectProviderARNs": [&provider_arn]}),
        )
        .await;
    let identity_id = emulator
        .ok("GetId", &json!({"IdentityPoolId": &pool_id}))
        .await["IdentityId"]
        .as_str()
        .expect("GetId answers an IdentityId")
        .to_string();
    let logins = json!({ issuer.as_str(): id_token(&issuer, json!(CLIENT_A)) });

    refused(
        &emulator,
        "before the client id is registered with the provider",
        &pool_id,
        &identity_id,
        &logins,
        INCORRECT_AUDIENCE_MESSAGE,
    )
    .await;

    emulator
        .iam_ok(
            "AddClientIDToOpenIDConnectProvider",
            &[
                (
                    "OpenIDConnectProviderArn".to_string(),
                    provider_arn.clone(),
                ),
                ("ClientID".to_string(), CLIENT_A.to_string()),
            ],
        )
        .await;
    admitted(
        &emulator,
        "after the client id is registered with the provider",
        &pool_id,
        &logins,
    )
    .await;

    emulator
        .iam_ok(
            "DeleteOpenIDConnectProvider",
            &[(
                "OpenIDConnectProviderArn".to_string(),
                provider_arn.clone(),
            )],
        )
        .await;
    // The pool still NAMES the provider's ARN, and that ARN names the issuer, so the pool
    // trusts the issuer still — with no registration left to accept any audience, which is
    // what the refusal states.
    refused(
        &emulator,
        "after the provider is deleted under the live pool",
        &pool_id,
        &identity_id,
        &logins,
        INCORRECT_AUDIENCE_MESSAGE,
    )
    .await;

    // Deleting the provider does not cascade into the pool: the ARN it names is still the
    // one a read of the pool reports, dangling.
    let described = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &pool_id}))
        .await;
    let named: Vec<&str> = described["OpenIdConnectProviderARNs"]
        .as_array()
        .unwrap_or_else(|| panic!("the pool reports its provider ARNs: {described}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        named,
        vec![provider_arn.as_str()],
        "the deleted provider's ARN must still be the one the pool names, got {described}"
    );

    // And registering the issuer again makes its tokens admissible again, because the
    // pool's trust is read live rather than copied into this service's own state.
    emulator.oidc_provider(&issuer, &[CLIENT_A]).await;
    admitted(
        &emulator,
        "after the provider is registered again",
        &pool_id,
        &logins,
    )
    .await;
}

/// RULE, as the POOL's own configuration changes: a provider ARN `UpdateIdentityPool` drops
/// states no trust any more, so logins of that issuer are refused — and a sibling provider
/// the pool still names keeps admitting its own.
#[tokio::test]
async fn a_provider_arn_dropped_by_update_identity_pool_stops_admitting_its_issuer() {
    let emulator = Emulator::new();
    let dropped_issuer = user_pool_issuer("us-east-1_dropped01");
    let kept_issuer = "auth.example.org/realms/kept".to_string();
    let dropped_arn = emulator.oidc_provider(&dropped_issuer, &[CLIENT_A]).await;
    let kept_arn = emulator.oidc_provider(&kept_issuer, &[CLIENT_B]).await;
    let pool_id = emulator
        .pool(
            "oidc-dropped",
            json!({"OpenIdConnectProviderARNs": [&dropped_arn, &kept_arn]}),
        )
        .await;

    let dropped_logins =
        json!({ dropped_issuer.as_str(): id_token(&dropped_issuer, json!(CLIENT_A)) });
    let kept_logins = json!({ kept_issuer.as_str(): id_token(&kept_issuer, json!(CLIENT_B)) });
    let dropped_identity = admitted(
        &emulator,
        "the provider later dropped, while the pool still names it",
        &pool_id,
        &dropped_logins,
    )
    .await;
    admitted(
        &emulator,
        "the sibling provider, before the update",
        &pool_id,
        &kept_logins,
    )
    .await;

    emulator
        .ok(
            "UpdateIdentityPool",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityPoolName": "oidc-dropped",
                "AllowUnauthenticatedIdentities": true,
                "OpenIdConnectProviderARNs": [&kept_arn],
            }),
        )
        .await;

    refused(
        &emulator,
        "the dropped provider's issuer, after the update",
        &pool_id,
        &dropped_identity,
        &dropped_logins,
        UNSUPPORTED_PROVIDER_MESSAGE,
    )
    .await;
    admitted(
        &emulator,
        "the sibling provider, after the update",
        &pool_id,
        &kept_logins,
    )
    .await;
}

/// RULE, on the two other operations the shared predicate decides: `GetOpenIdToken` and
/// `UnlinkIdentity` answer the same audience refusal for the same login, and a refused call
/// leaves the identity's links exactly as they were — while the token the trust DOES accept
/// is served.
#[tokio::test]
async fn the_audience_refusal_answers_get_open_id_token_and_unlink_identity_alike() {
    let emulator = Emulator::new();
    let issuer = user_pool_issuer("us-east-1_bothops01");
    let provider_arn = emulator.oidc_provider(&issuer, &[CLIENT_A]).await;
    let pool_id = emulator
        .pool(
            "oidc-both-operations",
            json!({"OpenIdConnectProviderARNs": [&provider_arn]}),
        )
        .await;

    let accepted = json!({ issuer.as_str(): id_token(&issuer, json!(CLIENT_A)) });
    let identity_id = admitted(
        &emulator,
        "the token the provider accepts",
        &pool_id,
        &accepted,
    )
    .await;
    let linked_before = described_logins(&emulator, "before the refusals", &identity_id).await;

    // The audience the provider does not accept, presented to each operation in the form
    // that operation takes.
    let refused_logins = json!({ issuer.as_str(): id_token(&issuer, json!(CLIENT_B)) });
    let rows: Vec<(&str, Value)> = vec![
        (
            "GetOpenIdToken",
            json!({"IdentityId": &identity_id, "Logins": &refused_logins}),
        ),
        (
            "UnlinkIdentity",
            json!({
                "IdentityId": &identity_id,
                "Logins": &refused_logins,
                "LoginsToRemove": [issuer.as_str()],
            }),
        ),
    ];
    for (operation, body) in &rows {
        let (status, payload) = emulator.call(operation, body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, NOT_AUTHORIZED),
            "case {operation}: expected (400, {NOT_AUTHORIZED}), got ({status}, {payload})"
        );
        assert_eq!(
            payload["message"].as_str(),
            Some(INCORRECT_AUDIENCE_MESSAGE),
            "case {operation}: expected the message {INCORRECT_AUDIENCE_MESSAGE:?}, got {payload}"
        );
        assert_eq!(
            payload.get("Token"),
            None,
            "case {operation}: a refusal carries no Token, got {payload}"
        );
        let linked = described_logins(&emulator, operation, &identity_id).await;
        assert_eq!(
            linked, linked_before,
            "case {operation}: a refused call must leave the identity's logins alone, got \
             {linked:?}"
        );
    }

    // The token the trust accepts is served by the same two operations: the predicate
    // decides admission, it does not close the operations.
    let token = emulator
        .ok(
            "GetOpenIdToken",
            &json!({"IdentityId": &identity_id, "Logins": &accepted}),
        )
        .await;
    assert!(
        token["Token"].as_str().is_some_and(|value| !value.is_empty()),
        "an accepted login must mint an OpenID token, got {token}"
    );
    emulator
        .ok(
            "UnlinkIdentity",
            &json!({
                "IdentityId": &identity_id,
                "Logins": &accepted,
                "LoginsToRemove": [issuer.as_str()],
            }),
        )
        .await;
}

/// The `Logins` list `DescribeIdentity` reports for one identity.
pub(crate) async fn described_logins(
    emulator: &Emulator,
    case: &str,
    identity_id: &str,
) -> Vec<String> {
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await;
    described["Logins"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: an identity carries Logins: {described}"))
        .iter()
        .map(|login| login.as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: `Logins` names a "provider_user_identifier", so an admitted login resolves to the
/// identity of the user its token NAMES — every token of one subject is that one user, and
/// another subject is another user.
///
/// The rows differ along what the rule generalises over: the app client the token was issued
/// for, the token's own bytes (a second token of the same user), the audience spelled as a
/// list, and the subject itself.
#[tokio::test]
async fn every_token_naming_one_subject_resolves_to_one_identity() {
    let emulator = Emulator::new();
    let issuer = user_pool_issuer("us-east-1_onesubject");
    let provider_arn = emulator
        .oidc_provider(&issuer, &[CLIENT_A, CLIENT_B])
        .await;
    let pool_id = emulator
        .pool(
            "oidc-one-identity",
            json!({"OpenIdConnectProviderARNs": [&provider_arn]}),
        )
        .await;

    // (case, the subject the token names, the token)
    let another_subject = "0a1b2c3d-0000-4000-8000-ba9876543210";
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "the first app client's token",
            SUBJECT,
            id_token_for(&issuer, json!(CLIENT_A), SUBJECT, "jti-1"),
        ),
        (
            "another token of the same client, same user",
            SUBJECT,
            id_token_for(&issuer, json!(CLIENT_A), SUBJECT, "jti-2"),
        ),
        (
            "the same user through a second app client",
            SUBJECT,
            id_token_for(&issuer, json!(CLIENT_B), SUBJECT, "jti-3"),
        ),
        (
            "the same user, audience spelled as a list",
            SUBJECT,
            id_token_for(&issuer, json!([CLIENT_B, CLIENT_A]), SUBJECT, "jti-4"),
        ),
        (
            "another user of the same issuer",
            another_subject,
            id_token_for(&issuer, json!(CLIENT_A), another_subject, "jti-5"),
        ),
        (
            "another token of that other user",
            another_subject,
            id_token_for(&issuer, json!(CLIENT_B), another_subject, "jti-6"),
        ),
    ];

    let mut resolved: Vec<(&str, &str, String)> = Vec::new();
    for (case, subject, token) in &rows {
        let logins = json!({ issuer.as_str(): token });
        let identity_id = admitted(&emulator, case, &pool_id, &logins).await;
        resolved.push((case, subject, identity_id));
    }

    for (case, subject, identity_id) in &resolved {
        for (other_case, other_subject, other_identity) in &resolved {
            let same_user = subject == other_subject;
            assert_eq!(
                identity_id == other_identity,
                same_user,
                "case {case:?} against {other_case:?}: tokens of the same subject must resolve \
                 to one identity and two subjects to two (same subject == {same_user}), got \
                 {identity_id} and {other_identity}"
            );
        }
    }
}
