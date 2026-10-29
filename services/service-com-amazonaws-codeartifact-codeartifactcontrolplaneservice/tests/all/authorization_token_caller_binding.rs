// SPDX-License-Identifier: CC0-1.0

//! `GetAuthorizationToken` against the CALLER: what its credentials make the token's
//! expiration, and what its identity policies make of the mint at all.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted — and the session and the role live in the STS and IAM stores of the SAME shared
//! engine, the stores those services' own handlers write, so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. `durationSeconds=0` sets the token's expiration to the expiration of the CALLER'S OWN
//!    temporary credentials — whatever that session's own expiry is — while every other
//!    accepted value keeps naming a period from issuance.
//! 2. Minting needs BOTH `codeartifact:GetAuthorizationToken` and the cross-service
//!    `sts:GetServiceBearerToken` in the caller's identity policies: a principal holding one
//!    without the other is refused `AccessDeniedException` (403) and is handed no token.
//! 3. The cross-service permission is an IDENTITY permission: granting it in the domain's
//!    own resource policy does not substitute for it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{auth_for, create_domain, Answer, ACCOUNT, REGION};

/// The role every session below is assumed from.
const ROLE: &str = "ca-publisher";

/// The access key id of the assumed-role session's credentials: an `ASIA` key, as STS mints
/// for a session.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How far an asserted `expiration` may sit from the instant it is compared with: the
/// wall-clock cost of the call itself, which is a handler invocation rather than a sleep.
const CLOCK_SLACK_SECONDS: f64 = 60.0;

/// The documented default lifetime of temporary credentials, which a `durationSeconds=0`
/// request falls back to when the caller presented no session this emulator holds
/// (`AssumeRole`'s `DurationSeconds`: "By default, the value is set to 3600 seconds").
const DEFAULT_CREDENTIAL_SECONDS: f64 = 3_600.0;

/// One emulator holding this service over one shared engine, with that engine kept so a
/// test can seed the STS and IAM stores the decision is read from.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Mint the assumed-role session [`SESSION_KEY_ID`] speaks for, as `sts:AssumeRole`
    /// records one, lasting `duration_seconds`. Returns the session's own expiry — the
    /// `Expiration` the caller was given for those credentials.
    fn given_session(&self, duration_seconds: i32) -> SystemTime {
        let now = SystemTime::now();
        let expires_at = now + Duration::from_secs(u64::try_from(duration_seconds).unwrap_or(0));
        let store = sts::service_state(&*self.engine).expect("the STS store");
        let mut sessions = store.lock();
        sessions.sessions.remove(SESSION_KEY_ID);
        sessions
            .insert(sts::Session {
                access_key_id: SESSION_KEY_ID.to_string(),
                created_at: now,
                expires_at,
                duration_seconds,
                identity: sts::Identity {
                    user_id: sts::role_id(ROLE),
                    account: ACCOUNT.to_string(),
                    arn: format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/publish-session"),
                },
                kind: sts::SessionKind::AssumedRole,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                external_id: None,
                role_arn: Some(role_arn()),
                revoked: false,
            })
            .expect("the session is recorded");
        expires_at
    }

    /// Hold the role itself, as `iam:CreateRole` does: its policies are read under the ARN
    /// the account holds it by.
    fn given_role(&self) {
        let store = iam::service_state(&*self.engine).expect("the IAM store");
        store.lock().roles.insert(
            iam::name_key(ROLE),
            service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
                path: "/".to_string(),
                role_name: ROLE.to_string(),
                role_id: sts::role_id(ROLE),
                arn: role_arn(),
                create_date: aws_smithy_types::DateTime::from_secs(0),
                assume_role_policy_document: None,
                description: None,
                max_session_duration: None,
                permissions_boundary: None,
                tags: None,
                role_last_used: None,
                source_role_template: None,
            },
        );
    }

    /// Attach `document` to the role as its inline policy, replacing whatever it carried —
    /// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
    /// `iam:DeleteRolePolicy` does.
    fn given_role_policy(&self, document: Option<&Value>) {
        let store = iam::service_state(&*self.engine).expect("the IAM store");
        let mut held = store.lock();
        match document {
            Some(document) => {
                held.role_inline_policies.insert(
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
            None => {
                held.role_inline_policies.remove(&iam::name_key(ROLE));
            }
        }
    }
}

/// The ARN the account holds the role by.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// `GetAuthorizationToken`, signed with the access key id `access_key_id` — which is what
/// names the session the caller speaks for.
async fn get_token_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    domain: &str,
    duration_seconds: Option<i64>,
) -> Answer {
    let mut uri = format!("/v1/authorization-token?domain={domain}");
    if let Some(duration) = duration_seconds {
        uri.push_str(&format!("&duration={duration}"));
    }
    let authorization = auth_for(REGION).replace("AKIDEXAMPLE", access_key_id);
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The `expiration` of an answer as epoch seconds (restJson1 serialises a `Timestamp` as an
/// epoch-seconds number).
fn expiration_of(answer: &Answer) -> Option<f64> {
    answer.body.get("expiration").and_then(Value::as_f64)
}

/// An instant as epoch seconds.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// RULE 1: `durationSeconds=0` expires the token with the caller's own session, whatever
/// that session's lifetime is, while every other accepted value is a period from issuance.
#[tokio::test]
async fn a_zero_duration_token_expires_with_the_session_that_minted_it() {
    let emulator = Emulator::new();
    let domain = "zero-duration";
    assert_eq!(
        create_domain(&emulator.router, domain, None).await.status,
        200,
        "CreateDomain must succeed before a token is requested"
    );
    emulator.given_role();
    emulator.given_role_policy(Some(&allow(json!([
        "codeartifact:GetAuthorizationToken",
        "sts:GetServiceBearerToken"
    ]))));

    // Rows: the session lifetimes a caller can hold — STS's own minimum, its default, and
    // the longest a role session can last — each asked for with `durationSeconds=0`.
    for (case, session_seconds) in [
        ("the shortest session STS mints", 900),
        ("the default session lifetime", 3_600),
        ("a twelve-hour session", 43_200),
    ] {
        let expires_at = emulator.given_session(session_seconds);
        let answer = get_token_as(&emulator.router, SESSION_KEY_ID, domain, Some(0)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let Some(expiration) = expiration_of(&answer) else {
            panic!("case {case:?}: expected an epoch-seconds expiration, got {answer:?}");
        };
        let expected = epoch_seconds(expires_at);
        assert!(
            (expiration - expected).abs() <= CLOCK_SLACK_SECONDS,
            "case {case:?}: expected the session's own expiration {expected}, got {expiration}"
        );

        // The control: a token that NAMES a period still expires that many seconds from
        // issuance, past the session's own expiry where the period is longer.
        let requested = 43_200_i64;
        let before = epoch_seconds(SystemTime::now());
        let named = get_token_as(&emulator.router, SESSION_KEY_ID, domain, Some(requested)).await;
        let Some(named_expiration) = expiration_of(&named) else {
            panic!("case {case:?}: expected an epoch-seconds expiration, got {named:?}");
        };
        let expected_named = before + requested as f64;
        assert!(
            (named_expiration - expected_named).abs() <= CLOCK_SLACK_SECONDS,
            "case {case:?}: expected a token naming {requested}s to expire at {expected_named}, \
             got {named_expiration}"
        );
    }

    // A caller presenting no session this emulator holds has no credential expiry to read,
    // so `durationSeconds=0` falls back to the documented default lifetime of temporary
    // credentials.
    let before = epoch_seconds(SystemTime::now());
    let answer = get_token_as(&emulator.router, "AKIDEXAMPLE", domain, Some(0)).await;
    assert_eq!(
        answer.status, 200,
        "case \"no session behind the credential\": expected 200, got {answer:?}"
    );
    let Some(expiration) = expiration_of(&answer) else {
        panic!("case \"no session behind the credential\": expected an expiration, got {answer:?}");
    };
    assert!(
        (expiration - (before + DEFAULT_CREDENTIAL_SECONDS)).abs() <= CLOCK_SLACK_SECONDS,
        "case \"no session behind the credential\": expected the documented default lifetime, \
         got {expiration}"
    );
}

/// RULES 2 and 3: the mint needs both permissions in the caller's identity policies, and a
/// domain resource policy granting the cross-service action does not substitute for it.
#[tokio::test]
async fn minting_needs_both_the_services_own_action_and_the_cross_service_bearer_action() {
    let emulator = Emulator::new();
    let domain = "both-permissions";
    assert_eq!(
        create_domain(&emulator.router, domain, None).await.status,
        200,
        "CreateDomain must succeed before a token is requested"
    );
    emulator.given_role();
    emulator.given_session(3_600);

    let both = allow(json!([
        "codeartifact:GetAuthorizationToken",
        "sts:GetServiceBearerToken"
    ]));
    let own_only = allow(json!("codeartifact:*"));
    let bearer_only = allow(json!("sts:GetServiceBearerToken"));
    let everything = allow(json!("*"));
    let mut denied = both.clone();
    denied["Statement"]
        .as_array_mut()
        .expect("a statement list")
        .push(json!({
            "Effect": "Deny",
            "Action": "sts:GetServiceBearerToken",
            "Resource": "*"
        }));

    // (case, the policy the role carries, the expected status, a fragment the refusal names)
    let cases: [(&str, Option<&Value>, u16, &str); 6] = [
        ("both permissions granted", Some(&both), 200, ""),
        (
            "this service's actions only",
            Some(&own_only),
            403,
            "sts:GetServiceBearerToken",
        ),
        (
            "the cross-service action only",
            Some(&bearer_only),
            403,
            "codeartifact:GetAuthorizationToken",
        ),
        ("every action of every service", Some(&everything), 200, ""),
        (
            "an explicit deny on the cross-service action",
            Some(&denied),
            403,
            "explicit deny",
        ),
        ("no policy at all", None, 403, "not authorized"),
    ];

    for (case, policy, expected, fragment) in cases {
        emulator.given_role_policy(policy);
        let answer = get_token_as(&emulator.router, SESSION_KEY_ID, domain, None).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
        if expected == 200 {
            assert!(
                answer
                    .body
                    .get("authorizationToken")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty()),
                "case {case:?}: expected a minted token, got {answer:?}"
            );
            continue;
        }
        assert_eq!(
            answer.code(),
            "AccessDeniedException",
            "case {case:?}: expected the modelled refusal, got {answer:?}"
        );
        assert!(
            answer.message().contains(fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {:?}",
            answer.message()
        );
        assert!(
            answer.body.get("authorizationToken").is_none(),
            "case {case:?}: a refused mint must hand out no token, got {answer:?}"
        );
    }

    // RULE 3: the domain's own resource policy granting the cross-service action to that
    // role changes nothing — the permission is the caller's to hold.
    emulator.given_role_policy(Some(&own_only));
    let attached = crate::harness::call(
        &emulator.router,
        "PUT",
        "/v1/domain/permissions/policy",
        Some(json!({
            "domain": domain,
            "policyDocument": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": role_arn() },
                    "Action": [
                        "codeartifact:GetAuthorizationToken",
                        "codeartifact:DescribeDomain",
                        "sts:GetServiceBearerToken"
                    ],
                    "Resource": "*"
                }]
            })
            .to_string()
        })),
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "PutDomainPermissionsPolicy must succeed, got {attached:?}"
    );
    let answer = get_token_as(&emulator.router, SESSION_KEY_ID, domain, None).await;
    assert_eq!(
        answer.status, 403,
        "case \"the domain policy grants the cross-service action\": expected the mint to stay \
         refused, got {answer:?}"
    );
}
