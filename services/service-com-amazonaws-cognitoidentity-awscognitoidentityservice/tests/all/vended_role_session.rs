// SPDX-License-Identifier: CC0-1.0

//! What the credentials an identity pool vends STAND FOR.
//!
//! RULE: `GetCredentialsForIdentity` vends the credentials of an
//! `sts:AssumeRoleWithWebIdentity` session of the role the pool resolves for the request —
//! its `authenticated` role for a request that presented a login, its `unauthenticated`
//! role for one that did not, or the `CustomRoleArn` the request names among the roles the
//! pool stores — so `sts:GetCallerIdentity` under the vended credential answers
//! `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`, and the
//! assume is recorded as STS's own `AssumeRoleWithWebIdentity` event. The assume happens
//! only where the resolved role admits it: a pool that names no role, a role the account
//! does not hold, and a role whose trust policy does not admit the Cognito Identity
//! principal for THIS pool (its `cognito-identity.amazonaws.com:aud` condition) are each
//! refused with the modelled `InvalidIdentityPoolConfigurationException`, with no
//! credentials and nothing recorded.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the production
//! router builds, with STS mounted beside this service on the ONE shared engine — so the
//! session a vend mints is resolved by the service that reports it, and not by a second
//! copy of the registry. The IAM records the decision reads are seeded into that same
//! engine, the store `iam:CreateRole` writes.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use service_common::management_events::ManagementEventLog;
use smithy_server_runtime::MultiServiceRouter;

/// The `RoleSessionName` every session a pool vends carries, as this service's state
/// states it — read from the crate rather than written again here.
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::VENDED_ROLE_SESSION_NAME;

/// The Region every call below is signed for, and so the Region the ids are minted in.
const REGION: &str = "us-east-1";

/// The Cognito Identity principal a role's trust policy names to admit an identity pool's
/// users, as `AssumeRoleWithWebIdentityRequest$RoleArn` documents it.
const COGNITO_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// One emulator: a shared engine with this service and STS mounted on it.
///
/// `pub(crate)` so a sibling suite in this test target that needs BOTH services on one
/// engine — the token this service mints being redeemed by the service that reports the
/// session — drives them through this harness rather than a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not verified: these suites carry only the `Credential=` scope the
        // services read the caller and the Region from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// One awsJson1_1 call to this service, as a client makes it.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            )
            .header("user-agent", "aws-sdk-rust/test")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=EMULATORTESTKEY12345/20260101/{REGION}/cognito-identity/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// A call that must succeed.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} on {body} expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create a pool and answer its minted `IdentityPoolId`.
    ///
    /// `pub(crate)` so a sibling suite on this harness asks for a pool the same way.
    pub(crate) async fn pool(&self, name: &str) -> String {
        self.ok(
            "CreateIdentityPool",
            &json!({"IdentityPoolName": name, "AllowUnauthenticatedIdentities": true}),
        )
        .await["IdentityPoolId"]
            .as_str()
            .expect("CreateIdentityPool answers an IdentityPoolId")
            .to_string()
    }

    /// Mint an identity in `pool_id`, with `logins` when it is federated.
    pub(crate) async fn identity(&self, pool_id: &str, logins: Option<Value>) -> String {
        let mut request = json!({"IdentityPoolId": pool_id});
        if let Some(logins) = logins {
            request["Logins"] = logins;
        }
        self.ok("GetId", &request).await["IdentityId"]
            .as_str()
            .expect("GetId answers an IdentityId")
            .to_string()
    }

    /// Seed `role_name` into the IAM store of the SAME shared engine, carrying
    /// `trust_policy` as its `AssumeRolePolicyDocument` — the record `iam:CreateRole`
    /// writes — and answer its ARN.
    ///
    /// The `RoleId` is seeded as its own value, so an assertion about the session's
    /// `AssumedRoleId` cannot pass against a handler that derives an id of its own.
    pub(crate) fn seed_role(&self, role_name: &str, trust_policy: Option<Value>) -> String {
        let arn = iam::state::role_arn(iam::state::DEFAULT_PATH, role_name);
        let role = iam::ops::types::Role {
            path: iam::state::DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROASEEDED{:0>11}", role_name.len()),
            arn: arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(1_767_225_600),
            assume_role_policy_document: trust_policy.map(|policy| policy.to_string()),
            description: None,
            max_session_duration: Some(43_200),
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        };
        self.engine
            .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>()
            .lock()
            .roles
            .insert(iam::state::name_key(role_name), role);
        arn
    }

    /// `sts:GetCallerIdentity` under `access_key_id`: an awsQuery POST, as a client makes
    /// it, carrying the key in the SigV4 credential scope.
    pub(crate) async fn caller_identity(&self, access_key_id: &str) -> (u16, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sts.amazonaws.com/")
            .header("host", "sts.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/sts/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                "Action=GetCallerIdentity&Version=2011-06-15",
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// Dispatch one request a suite built itself through the same router, and answer its
    /// status with the body as text.
    ///
    /// The seam a suite needs to make a call this harness's own helpers do not shape: an
    /// UNSIGNED `awsQuery` request to STS, the form a client holding no credential sends a
    /// modelled anonymous operation as.
    pub(crate) async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, String) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// Every event recorded in the account's management-event log, as the JSON document
    /// CloudTrail reports in an event's `CloudTrailEvent`.
    fn recorded_events(&self) -> Vec<Value> {
        self.engine
            .state::<ManagementEventLog>()
            .lock()
            .iter()
            .map(|event| event.to_cloudtrail_event_json())
            .collect()
    }

    /// The recorded events whose `eventName` is `event_name`, oldest first.
    fn recorded_named(&self, event_name: &str) -> Vec<Value> {
        self.recorded_events()
            .into_iter()
            .filter(|event| event.get("eventName") == Some(&json!(event_name)))
            .collect()
    }
}

/// The text of the one `<name>…</name>` element of an awsQuery response. Every element
/// these responses carry is uniquely named, so this reads a member without a document
/// model.
pub(crate) fn element(body: &str, name: &str) -> String {
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

/// The error code an awsJson error response carries.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error response carries a __type: {body}"))
        .to_string()
}

/// A trust policy that admits the Cognito Identity principal for the identity pool
/// `audience`, written the way the Amazon Cognito Developer Guide writes one: the
/// `Federated` principal, the web-identity assume, and the `:aud` condition that restricts
/// the assume to users of that one pool.
pub(crate) fn trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PRINCIPAL},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {"cognito-identity.amazonaws.com:aud": audience}},
        }],
    })
}

/// The assumed-role ARN a session of `role_name` speaks for.
pub(crate) fn expected_session_arn(role_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{VENDED_ROLE_SESSION_NAME}",
        engine::emulator_account_id()
    )
}

/// The `Credentials` of a served vend.
pub(crate) fn vended(response: &Value) -> (&str, &str) {
    let credentials = &response["Credentials"];
    for member in ["AccessKeyId", "SecretKey", "SessionToken"] {
        assert!(
            credentials[member]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "a vend carries a non-empty Credentials.{member}, got {response}"
        );
    }
    (
        credentials["AccessKeyId"].as_str().expect("an access key"),
        credentials["SessionToken"]
            .as_str()
            .expect("a session token"),
    )
}

/// RULE, served side: the credentials a vend hands back are a session of the role the pool
/// resolves for THAT request, whichever way the request resolves one.
#[tokio::test]
async fn a_vend_is_a_session_of_the_role_the_pool_resolves() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("vended-role").await;
    let authenticated = emulator.seed_role("vend-auth", Some(trust_policy(&pool_id)));
    let unauthenticated = emulator.seed_role("vend-unauth", Some(trust_policy(&pool_id)));
    let mapped = emulator.seed_role("vend-mapped", Some(trust_policy(&pool_id)));
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &authenticated, "unauthenticated": &unauthenticated},
                "RoleMappings": {
                    "accounts.google.com": {
                        "Type": "Rules",
                        "AmbiguousRoleResolution": "Deny",
                        "RulesConfiguration": {"Rules": [{
                            "Claim": "email",
                            "MatchType": "Equals",
                            "Value": "someone@example.com",
                            "RoleARN": &mapped,
                        }]},
                    },
                },
            }),
        )
        .await;

    // The rows differ along what the rule generalises over: which flow the request is in
    // (no login, one login, two logins) and whether it names a `CustomRoleArn` — one of
    // the pool's two roles, or a role one of its mapping rules resolves to.
    let rows: Vec<(&str, Option<Value>, Option<&str>, &str)> = vec![
        (
            "no Logins: the unauthenticated role",
            None,
            None,
            "vend-unauth",
        ),
        (
            "one login: the authenticated role",
            Some(json!({"graph.facebook.com": "a.token.value"})),
            None,
            "vend-auth",
        ),
        (
            "two logins: still the authenticated role",
            Some(json!({"accounts.google.com": "t1", "appleid.apple.com": "t2"})),
            None,
            "vend-auth",
        ),
        (
            // Naming a `CustomRoleArn` is something only an exchange whose provider offered
            // several roles may do, so this row presents a login of the provider the pool
            // holds a role mapping for.
            "a CustomRoleArn naming the pool's authenticated role",
            Some(json!({"accounts.google.com": "t4"})),
            Some(&authenticated),
            "vend-auth",
        ),
        (
            "a CustomRoleArn naming a role a mapping rule resolves to",
            Some(json!({"accounts.google.com": "t3"})),
            Some(&mapped),
            "vend-mapped",
        ),
    ];

    for (case, logins, custom_role_arn, role_name) in &rows {
        let identity_id = emulator.identity(&pool_id, logins.clone()).await;
        let mut request = json!({"IdentityId": &identity_id});
        if let Some(logins) = logins {
            request["Logins"] = logins.clone();
        }
        if let Some(custom_role_arn) = custom_role_arn {
            request["CustomRoleArn"] = json!(custom_role_arn);
        }
        let issued = emulator.ok("GetCredentialsForIdentity", &request).await;
        assert_eq!(
            issued["IdentityId"].as_str(),
            Some(identity_id.as_str()),
            "case {case:?}: a vend names the identity it was asked for, got {issued}"
        );
        let (access_key_id, _) = vended(&issued);

        let (status, body) = emulator.caller_identity(access_key_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCallerIdentity under the vended credential expected 200, \
             got {status} with {body}"
        );
        let arn = element(&body, "Arn");
        assert_eq!(
            arn,
            expected_session_arn(role_name),
            "case {case:?}: expected a session of {role_name}, got {arn}"
        );
        assert_eq!(
            element(&body, "Account"),
            engine::emulator_account_id(),
            "case {case:?}: the session belongs to the role's account, got {body}"
        );
        // `UserId` is `<RoleId>:<RoleSessionName>`, built from the id IAM issued for the
        // role, so the session correlates back to the role it is of.
        let user_id = element(&body, "UserId");
        assert!(
            user_id.starts_with("AROASEEDED") && user_id.ends_with(VENDED_ROLE_SESSION_NAME),
            "case {case:?}: expected <RoleId>:{VENDED_ROLE_SESSION_NAME}, got {user_id}"
        );

        // Asked again inside the window, the identity has the same session.
        let again = emulator.ok("GetCredentialsForIdentity", &request).await;
        assert_eq!(
            again, issued,
            "case {case:?}: a second vend inside the window answered {again}, expected \
             {issued}"
        );
    }
}

/// RULE, refused side: a pool whose resolved role does not admit an assume by ITS users
/// vends nothing — whatever it is about the role that withholds the grant.
#[tokio::test]
async fn a_role_that_does_not_admit_this_pool_vends_nothing() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("refused-role").await;
    let other_pool_id = emulator.pool("another-pool").await;
    let identity_id = emulator.identity(&pool_id, None).await;

    // One row per reason the resolved role withholds the grant. `None` as the policy is a
    // role IAM holds with no trust policy at all.
    let rows: Vec<(&str, Option<Value>)> = vec![
        (
            "a trust policy conditioning the assume on ANOTHER identity pool",
            Some(trust_policy(&other_pool_id)),
        ),
        (
            "an explicit deny for this pool",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"Federated": COGNITO_PRINCIPAL},
                    "Action": "sts:AssumeRoleWithWebIdentity",
                    "Condition": {"StringEquals": {"cognito-identity.amazonaws.com:aud": &pool_id}},
                }],
            })),
        ),
        (
            "a trust policy granting only sts:AssumeRole",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Federated": COGNITO_PRINCIPAL},
                    "Action": "sts:AssumeRole",
                }],
            })),
        ),
        (
            "a trust policy naming another federated principal",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Federated": "accounts.google.com"},
                    "Action": "sts:AssumeRoleWithWebIdentity",
                }],
            })),
        ),
        (
            "a trust policy requiring an authenticated exchange, for an unauthenticated one",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Federated": COGNITO_PRINCIPAL},
                    "Action": "sts:AssumeRoleWithWebIdentity",
                    "Condition": {
                        "StringEquals": {"cognito-identity.amazonaws.com:aud": &pool_id},
                        "ForAnyValue:StringLike": {
                            "cognito-identity.amazonaws.com:amr": "authenticated",
                        },
                    },
                }],
            })),
        ),
        ("a role carrying no trust policy at all", None),
    ];

    for (index, (case, policy)) in rows.iter().enumerate() {
        let role_name = format!("refused-{index}");
        let role_arn = emulator.seed_role(&role_name, policy.clone());
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"unauthenticated": &role_arn},
                }),
            )
            .await;

        let (status, body) = emulator
            .call(
                "GetCredentialsForIdentity",
                &json!({"IdentityId": &identity_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidIdentityPoolConfigurationException"),
            "case {case:?}: expected InvalidIdentityPoolConfigurationException, got {body}"
        );
        assert_eq!(
            body.get("Credentials"),
            None,
            "case {case:?}: a refused vend carries no Credentials, got {body}"
        );
    }

    // And the two configurations that resolve no role at all: a role ARN the account holds
    // no record of, and a pool that names no role for the request.
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {
                    "unauthenticated": iam::state::role_arn(iam::state::DEFAULT_PATH, "never-created"),
                },
            }),
        )
        .await;
    for case in ["a role ARN the account holds no role for", "no role at all"] {
        let (status, body) = emulator
            .call(
                "GetCredentialsForIdentity",
                &json!({"IdentityId": &identity_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidIdentityPoolConfigurationException"),
            "case {case:?}: expected InvalidIdentityPoolConfigurationException, got {body}"
        );
        // Replace the configuration with an EMPTY one for the second row.
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({"IdentityPoolId": &pool_id, "Roles": {}}),
            )
            .await;
    }

    // Nothing was assumed, so no assume was recorded: the failed-assume path of a real
    // account records no assume event, successful or failed. The vend itself is an identity
    // data-plane call, which CloudTrail delivers no management event for either, so a
    // refused vend leaves the log empty.
    let assumes = emulator.recorded_named("AssumeRoleWithWebIdentity");
    assert!(
        assumes.is_empty(),
        "a refused vend records no assume, got {assumes:?}"
    );
    let vends = emulator.recorded_named("GetCredentialsForIdentity");
    assert!(
        vends.is_empty(),
        "a vend is a data event and is recorded nowhere, got {vends:?}"
    );
}

/// RULE, audit side: a served vend is recorded as STS's own `AssumeRoleWithWebIdentity`
/// event, naming the role it assumed, the pool it was assumed for and the identity it was
/// assumed on behalf of — and carrying no secret.
#[tokio::test]
async fn a_served_vend_is_audited_as_an_sts_web_identity_assume() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("audited-vend").await;
    let role_arn = emulator.seed_role("audited-role", Some(trust_policy(&pool_id)));
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
            }),
        )
        .await;

    // Two vends, in the two flows, so the log is asserted over more than one row.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("an unauthenticated vend", None),
        (
            "an authenticated vend",
            Some(json!({"graph.facebook.com": "a.token.value"})),
        ),
    ];
    for (case, logins) in &rows {
        let identity_id = emulator.identity(&pool_id, logins.clone()).await;
        let mut request = json!({"IdentityId": &identity_id});
        if let Some(logins) = logins {
            request["Logins"] = logins.clone();
        }
        let issued = emulator.ok("GetCredentialsForIdentity", &request).await;
        let (access_key_id, session_token) = vended(&issued);

        let recorded = emulator.recorded_named("AssumeRoleWithWebIdentity");
        let event = recorded
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: the vend recorded no assume"));
        for (field, expected) in [
            ("eventName", json!("AssumeRoleWithWebIdentity")),
            ("eventSource", json!("sts.amazonaws.com")),
            ("awsRegion", json!(REGION)),
            ("readOnly", json!(false)),
        ] {
            assert_eq!(
                event.get(field),
                Some(&expected),
                "case {case:?}: expected {field} {expected}, got {event}"
            );
        }
        assert_eq!(
            event.pointer("/requestParameters/roleArn"),
            Some(&json!(role_arn)),
            "case {case:?}: the record names the role assumed, got {event}"
        );
        assert_eq!(
            event.pointer("/requestParameters/roleSessionName"),
            Some(&json!(VENDED_ROLE_SESSION_NAME)),
            "case {case:?}: the record names the session, got {event}"
        );
        // The assume was made for a user a web identity provider authenticated, and the
        // provider that authenticated them is the identity pool's own service principal.
        assert_eq!(
            event.pointer("/userIdentity/type"),
            Some(&json!("WebIdentityUser")),
            "case {case:?}: the record names a web-identity caller, got {event}"
        );
        assert_eq!(
            event.pointer("/userIdentity/identityProvider"),
            Some(&json!(COGNITO_PRINCIPAL)),
            "case {case:?}: the record names the provider that authenticated the user, \
             got {event}"
        );
        assert_eq!(
            event.pointer("/userIdentity/principalId"),
            Some(&json!(format!(
                "{COGNITO_PRINCIPAL}:{pool_id}:{identity_id}"
            ))),
            "case {case:?}: the record names the principal the token identified, got \
             {event}"
        );
        assert_eq!(
            event.pointer("/userIdentity/userName"),
            Some(&json!(identity_id)),
            "case {case:?}: the record names the identity the assume was made for, got \
             {event}"
        );
        assert_eq!(
            event.pointer("/responseElements/audience"),
            Some(&json!(pool_id)),
            "case {case:?}: the record names the pool the assume was for, got {event}"
        );
        assert_eq!(
            event.pointer("/responseElements/subjectFromWebIdentityToken"),
            Some(&json!(identity_id)),
            "case {case:?}: the record names the identity, got {event}"
        );
        assert_eq!(
            event.pointer("/responseElements/assumedRoleUser/arn"),
            Some(&json!(expected_session_arn("audited-role"))),
            "case {case:?}: the record names the session's ARN, got {event}"
        );
        assert_eq!(
            event.pointer("/responseElements/credentials/accessKeyId"),
            Some(&json!(access_key_id)),
            "case {case:?}: the record names the credential minted, got {event}"
        );
        let document = event.to_string();
        assert!(
            !document.contains(session_token),
            "case {case:?}: an audit record must carry no session token, got {event}"
        );
        assert!(
            !document.contains(
                issued["Credentials"]["SecretKey"]
                    .as_str()
                    .expect("a secret key")
            ),
            "case {case:?}: an audit record must carry no secret key, got {event}"
        );
    }

    let recorded = emulator.recorded_named("AssumeRoleWithWebIdentity");
    assert_eq!(
        recorded.len(),
        rows.len(),
        "each vend records exactly one assume, got {recorded:?}"
    );
    // The vend ITSELF is an identity data-plane call, which CloudTrail delivers no
    // management event for: only the assume it made is in the log.
    let vends = emulator.recorded_named("GetCredentialsForIdentity");
    assert!(
        vends.is_empty(),
        "a vend is a data event and is recorded nowhere, got {vends:?}"
    );
}
