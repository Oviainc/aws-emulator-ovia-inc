// SPDX-License-Identifier: CC0-1.0

//! The entry points a client holding NO Amazon Web Services credentials calls, driven
//! through the WHOLE assembled router with request authentication ON — the configuration
//! the emulator serves traffic in.
//!
//! RULE: a request that carries no SigV4 credential scope is routed by what the request
//! itself names — the service prefix of an `awsJson` call's `X-Amz-Target`, or the endpoint
//! host — so every operation whose model lists no auth scheme is claimed by the service it
//! names and answered by that service's own handler, never by the router's `UnknownService`
//! (no service claimed the request) or `AmbiguousService` (several did). Claiming such a
//! request opens no door: an operation whose model DOES list SigV4 is still refused when it
//! arrives unsigned, and a target naming no mounted service still reaches nobody.
//!
//! The rule belongs to the router rather than to one service, so the table below spans
//! three services and both of the wire forms an anonymous operation is sent in — the
//! `awsJson1_1` calls of Amazon Cognito Identity and Amazon Cognito Identity Provider,
//! named by their target prefix, and STS's `awsQuery` call, named by its endpoint host
//! alone. Each service crate's own suite (e.g.
//! `service-com-amazonaws-cognitoidentity-awscognitoidentityservice`'s
//! `anonymous_credential_vend`) pins the same rule with ONLY that service mounted, which
//! cannot see a second claimant or a peer that swallows the request; this suite is that
//! rule over the assembled set, and it is the only one that runs with authentication on.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is addressed in.
const REGION: &str = "us-east-1";

/// The emulator's own address, which is what a client is pointed at: an address, so the
/// `Host` header names no service and only the target prefix can route the request.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id the signed setup calls carry.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// STS's awsQuery API version, and IAM's.
const STS_API_VERSION: &str = "2011-06-15";
const IAM_API_VERSION: &str = "2010-05-08";

/// The `X-Amz-Target` service prefixes of the two Cognito services: the local names of
/// their Smithy shapes, `com.amazonaws.cognitoidentity#AWSCognitoIdentityService` and
/// `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`.
const COGNITO_IDENTITY: &str = "AWSCognitoIdentityService";
const COGNITO_IDP: &str = "AWSCognitoIdentityProviderService";

/// The router's own refusals: nobody claimed the request, and several did. Neither is a
/// service's answer, and no row below may be answered either one.
const UNKNOWN_SERVICE: &str = "UnknownService";
const AMBIGUOUS_SERVICE: &str = "AmbiguousService";
const ROUTER_REFUSALS: [&str; 2] = [UNKNOWN_SERVICE, AMBIGUOUS_SERVICE];

/// The code a request refused by request authentication carries.
const UNAUTHENTICATED: &str = "AccessDenied";

/// The `RoleSessionName` a session an identity pool vends carries, and the one the
/// unsigned exchange below names.
const SESSION_NAME: &str = "CognitoIdentityCredentials";

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

/// How a request is written on the wire.
#[derive(Clone)]
enum Wire {
    /// An `awsJson1_1` call: a JSON body, with the operation — and its service prefix — in
    /// `X-Amz-Target`, or no target at all.
    Json { target: Option<String>, body: Value },
    /// An `awsQuery` call, which is STS's and IAM's wire form: a form-encoded body whose
    /// `Action` names the operation. The protocol publishes no target header, so an
    /// unsigned one is named by its endpoint host alone.
    Query { body: String },
}

/// A service's regional endpoint host, which names the service in the host tier.
fn endpoint(signing_name: &str) -> String {
    format!("{signing_name}.{REGION}.amazonaws.com")
}

/// Dispatch one request to `host`, carrying a credential scope for `scope` or none at all.
fn call(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    host: &str,
    scope: Option<&str>,
    wire: &Wire,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host);
    if let Some(signing_name) = scope {
        // The signature is never verified while the setup calls run: authentication is
        // switched on only once the pool, the role and the identity exist. What the scope
        // carries is the key id and the Region a handler reads.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={SETUP_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    let body = match wire {
        Wire::Json { target, body } => {
            builder = builder.header("content-type", "application/x-amz-json-1.1");
            if let Some(target) = target {
                builder = builder.header("x-amz-target", target.as_str());
            }
            bytes::Bytes::from(body.to_string())
        }
        Wire::Query { body } => {
            builder = builder.header("content-type", "application/x-www-form-urlencoded");
            bytes::Bytes::from(body.clone())
        }
    };
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// An `awsJson1_1` call to `target`, unsigned unless `scope` names a signing name.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    host: &str,
    scope: Option<&str>,
    target: &str,
    body: &Value,
) -> Answer {
    let wire = Wire::Json {
        target: Some(target.to_string()),
        body: body.clone(),
    };
    call(router, runtime, host, scope, &wire)
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The value an answer reports at `path`: the JSON pointer in a JSON body, and the text of
/// the element its last segment names in an XML one — the two envelopes the rows are
/// answered in.
fn reported(answer: &Answer, path: &str) -> Option<String> {
    let payload = answer.json();
    if payload.is_object() {
        return payload
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let element = path.split('/').next_back().unwrap_or(path);
    xml_element(&answer.text(), element)
}

/// The wire error code an answer carries, and `None` for one that carries none — a success,
/// which is already the answer of the service that claimed the request.
fn error_code(answer: &Answer) -> Option<String> {
    let payload = answer.json();
    if let Some(code) = payload.get("__type").and_then(Value::as_str) {
        // An `awsJson` error names its shape, which may carry the model's namespace.
        return Some(code.split('#').next_back().unwrap_or(code).to_string());
    }
    xml_element(&answer.text(), "Code")
}

/// What an unsigned call owes.
#[derive(Clone, Copy)]
enum Owed {
    /// HTTP 200, reporting a value at this path: an implemented operation, so the claim
    /// shows as the operation's own output.
    Served(&'static str),
    /// Answered by the service the request names, whatever that answer is. An operation
    /// whose own preconditions this request does not meet answers its own modelled error,
    /// which is still that service's answer and still proves the claim; pinning a status
    /// here would pin that operation's semantics instead of the routing rule, and would go
    /// stale the day it answers something else.
    Reached,
    /// Claimed, and then refused because the operation's model lists SigV4 and the request
    /// carries no signature.
    Unauthenticated,
    /// Claimed by nobody: nothing in the request names a mounted service.
    Unclaimed,
}

/// One row: how the unsigned request is addressed and written, and what it owes.
struct Row {
    case: &'static str,
    host: String,
    wire: Wire,
    owed: Owed,
}

/// A role trust policy admitting the Amazon Cognito Identity principal for the identity
/// pool `audience`, written the way the Amazon Cognito Developer Guide writes one: the
/// `Federated` principal, the web-identity assume, and the `:aud` condition restricting the
/// assume to users of that one pool.
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

#[test]
fn an_unsigned_request_is_answered_by_the_service_it_names() {
    std::thread::Builder::new()
        .name("unsigned-entry-points".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // SETUP, with authentication off so the signed calls below need no real
            // signature: an identity pool, a role its users may assume, and the pool
            // configured with it.
            router.set_request_authentication(false);
            let created = rpc(
                &router,
                &runtime,
                &endpoint("cognito-identity"),
                Some("cognito-identity"),
                "AWSCognitoIdentityService.CreateIdentityPool",
                &json!({
                    "IdentityPoolName": "unsigned-entry-points",
                    "AllowUnauthenticatedIdentities": true,
                    // The flow `GetOpenIdToken` belongs to.
                    "AllowClassicFlow": true,
                }),
            );
            assert_eq!(
                created.status,
                200,
                "setup: CreateIdentityPool must succeed, got {} with {:?}",
                created.status,
                created.text()
            );
            let pool_id = created.json()["IdentityPoolId"]
                .as_str()
                .expect("CreateIdentityPool reports an IdentityPoolId")
                .to_string();

            let role_name = "unsigned-entry-points-unauth";
            let role = call(
                &router,
                &runtime,
                &endpoint("iam"),
                Some("iam"),
                &Wire::Query {
                    body: format!(
                        "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={role_name}\
                         &AssumeRolePolicyDocument={}",
                        trust_policy(&pool_id)
                    ),
                },
            );
            assert_eq!(
                role.status,
                200,
                "setup: CreateRole must succeed, got {} with {:?}",
                role.status,
                role.text()
            );
            let role_arn = xml_element(&role.text(), "Arn").expect("CreateRole reports an Arn");

            let configured = rpc(
                &router,
                &runtime,
                &endpoint("cognito-identity"),
                Some("cognito-identity"),
                "AWSCognitoIdentityService.SetIdentityPoolRoles",
                &json!({"IdentityPoolId": &pool_id, "Roles": {"unauthenticated": &role_arn}}),
            );
            assert_eq!(
                configured.status,
                200,
                "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
                configured.status,
                configured.text()
            );

            // An identity, and a web identity token for it: both minted through the
            // unsigned entry points, which is the only way a client holding no credential
            // can reach them, and both re-driven as rows below once authentication is on.
            let minted = rpc(
                &router,
                &runtime,
                EMULATOR_HOST,
                None,
                "AWSCognitoIdentityService.GetId",
                &json!({"IdentityPoolId": &pool_id}),
            );
            assert_eq!(
                minted.status,
                200,
                "setup: an unsigned GetId must be served, got {} with {:?}",
                minted.status,
                minted.text()
            );
            let identity_id = minted.json()["IdentityId"]
                .as_str()
                .expect("GetId reports an IdentityId")
                .to_string();
            let issued = rpc(
                &router,
                &runtime,
                EMULATOR_HOST,
                None,
                "AWSCognitoIdentityService.GetOpenIdToken",
                &json!({"IdentityId": &identity_id}),
            );
            assert_eq!(
                issued.status,
                200,
                "setup: an unsigned GetOpenIdToken must be served, got {} with {:?}",
                issued.status,
                issued.text()
            );
            let token = issued.json()["Token"]
                .as_str()
                .expect("GetOpenIdToken reports a Token")
                .to_string();

            // From here on, request authentication is ON, as the emulator serves traffic:
            // every row below carries no signature at all.
            router.set_request_authentication(true);

            let rows = vec![
                Row {
                    case: "cognito-identity GetId, targeted at it on the emulator's address",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.GetId")),
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Served("/IdentityId"),
                },
                Row {
                    case: "cognito-identity GetId, addressed by its own endpoint host",
                    host: endpoint("cognito-identity"),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.GetId")),
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Served("/IdentityId"),
                },
                Row {
                    // A routing key is normalised, so the prefix routes however it is cased.
                    case: "cognito-identity GetId, lower-cased target prefix",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{}.GetId", COGNITO_IDENTITY.to_lowercase())),
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Served("/IdentityId"),
                },
                Row {
                    case: "cognito-identity GetCredentialsForIdentity: the session the \
                           client signs its next calls with",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.GetCredentialsForIdentity")),
                        body: json!({"IdentityId": &identity_id}),
                    },
                    owed: Owed::Served("/Credentials/AccessKeyId"),
                },
                Row {
                    case: "cognito-identity GetOpenIdToken",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.GetOpenIdToken")),
                        body: json!({"IdentityId": &identity_id}),
                    },
                    owed: Owed::Served("/Token"),
                },
                Row {
                    case: "cognito-identity UnlinkIdentity",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.UnlinkIdentity")),
                        body: json!({
                            "IdentityId": &identity_id,
                            "Logins": {"graph.facebook.com": "a.token.value"},
                            "LoginsToRemove": ["graph.facebook.com"],
                        }),
                    },
                    owed: Owed::Reached,
                },
                Row {
                    // Another service's anonymous entry point, on the same router: the rule
                    // is the router's, so it holds for every service with one.
                    case: "cognito-idp InitiateAuth, the user-pools service's own anonymous \
                           entry point",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDP}.InitiateAuth")),
                        body: json!({
                            "AuthFlow": "USER_PASSWORD_AUTH",
                            "ClientId": "unsignedentrypointsclient",
                            "AuthParameters": {"USERNAME": "someone", "PASSWORD": "a-password"},
                        }),
                    },
                    owed: Owed::Reached,
                },
                Row {
                    // The other wire form: `awsQuery` carries no target, so the endpoint
                    // host is the only thing naming the service.
                    case: "sts AssumeRoleWithWebIdentity, addressed by STS's endpoint host",
                    host: endpoint("sts"),
                    wire: Wire::Query {
                        body: format!(
                            "Action=AssumeRoleWithWebIdentity&Version={STS_API_VERSION}\
                             &RoleArn={role_arn}&RoleSessionName={SESSION_NAME}\
                             &WebIdentityToken={token}"
                        ),
                    },
                    owed: Owed::Served("/Credentials/AccessKeyId"),
                },
                Row {
                    // The claim is not a door: this operation's model lists SigV4, so an
                    // unsigned call is refused by authentication AFTER being claimed —
                    // never answered as a request that reached nobody.
                    case: "cognito-identity DescribeIdentityPool, an operation its model signs",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some(format!("{COGNITO_IDENTITY}.DescribeIdentityPool")),
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Unauthenticated,
                },
                Row {
                    case: "a target naming no mounted service",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: Some("AWSNoSuchService.GetId".to_string()),
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Unclaimed,
                },
                Row {
                    case: "no routing signal at all",
                    host: EMULATOR_HOST.to_string(),
                    wire: Wire::Json {
                        target: None,
                        body: json!({"IdentityPoolId": &pool_id}),
                    },
                    owed: Owed::Unclaimed,
                },
            ];

            for row in &rows {
                let answer = call(&router, &runtime, &row.host, None, &row.wire);
                let body = answer.text();
                let code = error_code(&answer);
                let status = answer.status;
                match row.owed {
                    Owed::Served(path) => {
                        assert_eq!(
                            status, 200,
                            "case {:?}: an unsigned call its model serves must be served, \
                             got {status} with {body:?}",
                            row.case
                        );
                        let value = reported(&answer, path);
                        assert!(
                            value.as_deref().is_some_and(|value| !value.is_empty()),
                            "case {:?}: expected {path} to be reported, got {body:?}",
                            row.case
                        );
                    }
                    Owed::Reached => {
                        let refused = code
                            .as_deref()
                            .is_some_and(|code| ROUTER_REFUSALS.contains(&code));
                        assert!(
                            !refused,
                            "case {:?}: an unsigned call must reach the service it names, \
                             got {status} with {body:?}",
                            row.case
                        );
                        assert_ne!(
                            code.as_deref(),
                            Some(UNAUTHENTICATED),
                            "case {:?}: an operation whose model lists no auth scheme must \
                             not be refused for carrying no signature, got {status} with \
                             {body:?}",
                            row.case
                        );
                    }
                    Owed::Unauthenticated => {
                        assert_eq!(
                            (status, code.as_deref()),
                            (403, Some(UNAUTHENTICATED)),
                            "case {:?}: expected the authentication refusal, got {status} \
                             with {body:?}",
                            row.case
                        );
                    }
                    Owed::Unclaimed => {
                        assert_eq!(
                            (status, code.as_deref()),
                            (404, Some(UNKNOWN_SERVICE)),
                            "case {:?}: expected the router's own refusal for a request no \
                             service claimed, got {status} with {body:?}",
                            row.case
                        );
                    }
                }
                assert_ne!(
                    code.as_deref(),
                    Some(AMBIGUOUS_SERVICE),
                    "case {:?}: exactly one service may claim a request, got {status} with \
                     {body:?}",
                    row.case
                );
            }
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the suite runs to completion");
}
