// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION of b2bi requests, driven over the wire
//! (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same router the emulator
//! boots.
//!
//! The rule under test: a request signed with credentials that name an assumed-role
//! session is authorized against the IAM identity policies of THAT role, in IAM's own
//! order — an explicit `Deny` matching the action and the resource refuses the request
//! with `AccessDeniedException` (HTTP 403) and performs nothing, whatever any `Allow`
//! says, and with no matching `Allow` the request is refused too (IAM's default deny). A
//! request whose access key id names no session speaks for the ambient identity, the
//! long-term key this emulator publishes, which is the account's own administrator.
//!
//! The role, its policies and the session all live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{call, call_as, error_code, ACCOUNT};

/// The role every session below is assumed from.
pub(crate) const ROLE: &str = "b2bi-onboarding";

/// The name of the session the role is assumed under, so the refusal a denied request
/// answers with names the session ARN `sts:GetCallerIdentity` reports.
pub(crate) const SESSION_NAME: &str = "onboarding-session";

/// The access key id of the assumed-role session's credentials.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// One emulator holding b2bi (and the CloudWatch Logs service `CreateProfile` provisions a
/// log group through) over one shared engine, with that engine kept so a test can seed the
/// IAM and STS stores the authorization decision is read from.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_b2bi_b2bi::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Mint the assumed-role session [`SESSION_KEY_ID`] speaks for, as `sts:AssumeRole`
    /// records one: the identity is the `assumed-role` ARN `sts:GetCallerIdentity` reports.
    pub(crate) fn given_session(&self) {
        let now = SystemTime::now();
        let vend = sts::NewSession {
            role_arn: Some(format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")),
            ..sts::NewSession::new(
                sts::Identity {
                    user_id: sts::role_id(ROLE),
                    account: ACCOUNT.to_string(),
                    arn: format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION_NAME}"),
                },
                sts::SessionKind::AssumedRole,
                SESSION_SECONDS,
            )
        };
        let store = sts::service_state(&*self.engine).expect("the STS store");
        store
            .lock()
            .insert(vend.into_session(SESSION_KEY_ID, now))
            .expect("the session is recorded");
    }

    /// Attach `document` to the role as its inline policy, replacing whatever it carried —
    /// what `iam:PutRolePolicy` does.
    ///
    /// A `document` that is a JSON STRING is stored verbatim rather than as its JSON text,
    /// because that is how a percent-encoded document — the form IAM reports a policy in —
    /// and a document that is not a policy at all reach the store.
    pub(crate) fn given_role_policy(&self, document: &Value) {
        let policy_document = match document.as_str() {
            Some(verbatim) => verbatim.to_string(),
            None => document.to_string(),
        };
        let store = iam::service_state(&*self.engine).expect("the IAM store");
        store.lock().role_inline_policies.insert(
            iam::name_key(ROLE),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document,
                },
            )]),
        );
    }

    /// Take every policy off the role — what `iam:DeleteRolePolicy` does.
    pub(crate) fn given_no_role_policy(&self) {
        let store = iam::service_state(&*self.engine).expect("the IAM store");
        store.lock().role_inline_policies.remove(&iam::name_key(ROLE));
    }

    /// Attach the managed policy `arn` to the role — what `iam:AttachRolePolicy` does.
    fn given_attached_policy(&self, arn: &str) {
        let store = iam::service_state(&*self.engine).expect("the IAM store");
        store
            .lock()
            .role_attached_policies
            .entry(iam::name_key(ROLE))
            .or_default()
            .insert(arn.to_string());
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// The request body `CreateProfile` is called with throughout.
fn create_profile() -> Value {
    json!({
        "name": "least-privilege",
        "phone": "555-555-0101",
        "businessName": "Least Privilege Corp",
        "logging": "DISABLED"
    })
}

/// How many profiles this account holds, read through a call the policies below always
/// allow, so a refusal can be checked to have created nothing.
async fn profile_count(emulator: &Emulator) -> usize {
    let (status, listed) = call(&emulator.router, "ListProfiles", json!({})).await;
    assert_eq!(status, 200, "ListProfiles must succeed, got {status} {listed}");
    listed["profiles"].as_array().map(Vec::len).unwrap_or_default()
}

/// RULE: an explicit `Deny` on an action refuses that action for the principal whose role
/// carries it — with `AccessDeniedException` (403), and creating nothing — while every
/// other action the policy allows keeps working; removing the `Deny` restores it.
#[tokio::test]
async fn an_explicit_deny_refuses_the_action_and_lifting_it_restores_access() {
    let emulator = Emulator::new();
    emulator.given_session();

    let allowed = allow(json!([
        "b2bi:CreateProfile",
        "b2bi:GetProfile",
        "b2bi:ListProfiles",
        "b2bi:DeleteProfile"
    ]));
    let mut denied = allowed.clone();
    denied["Statement"]
        .as_array_mut()
        .expect("a statement list")
        .push(json!({ "Effect": "Deny", "Action": "b2bi:CreateProfile", "Resource": "*" }));

    // Each row: the case, the policy the role carries, the expected status, and how many
    // profiles the account must hold afterwards.
    let cases: Vec<(&str, &Value, u16, usize)> = vec![
        ("an Allow-only policy", &allowed, 200, 1),
        ("an explicit Deny added", &denied, 403, 1),
        ("the Deny removed again", &allowed, 200, 2),
    ];

    for (case, policy, expected, profiles) in cases {
        emulator.given_role_policy(policy);
        let (status, response) = call_as(
            &emulator.router,
            SESSION_KEY_ID,
            "CreateProfile",
            create_profile(),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if expected == 403 {
            assert_eq!(
                error_code(&response),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {response}"
            );
            let message = response["message"].as_str().unwrap_or_default();
            for expected_fragment in [ROLE, "b2bi:CreateProfile", "explicit deny"] {
                assert!(
                    message.contains(expected_fragment),
                    "case {case:?}: the refusal must name {expected_fragment:?}, got \
                     {message:?}"
                );
            }
        }
        assert_eq!(
            profile_count(&emulator).await,
            profiles,
            "case {case:?}: the account must hold {profiles} profile(s) afterwards"
        );
    }
}

/// RULE: the decision follows the role's identity policies in IAM's own order — a `Deny`
/// wins, an `Allow` matching the action AND the resource permits, and nothing matching is
/// IAM's default deny. The rows differ along what the rule generalises over: the action
/// pattern's wildcards, `NotAction`, the resource the statement is scoped to, the
/// statement's effect and a statement this emulator does not evaluate.
#[tokio::test]
async fn a_request_is_authorized_against_the_roles_identity_policies() {
    let emulator = Emulator::new();
    emulator.given_session();

    // A profile to read, created while everything is allowed, plus its ARN so a row can
    // scope a statement to it.
    emulator.given_role_policy(&allow(json!("b2bi:*")));
    let (status, created) = call_as(
        &emulator.router,
        SESSION_KEY_ID,
        "CreateProfile",
        create_profile(),
    )
    .await;
    assert_eq!(status, 200, "the profile must be created, got {status} {created}");
    let profile_id = created["profileId"].as_str().unwrap_or_default().to_string();
    let profile_arn = created["profileArn"].as_str().unwrap_or_default().to_string();

    // Each row: the case, the policy the role carries (None takes every policy off it),
    // and the expected status of `GetProfile` on that profile.
    let cases: Vec<(&str, Option<Value>, u16)> = vec![
        ("no policy at all is IAM's default deny", None, 403),
        (
            "the exact action",
            Some(allow(json!("b2bi:GetProfile"))),
            200,
        ),
        (
            "every action of this service",
            Some(allow(json!("b2bi:*"))),
            200,
        ),
        ("every action of every service", Some(allow(json!("*"))), 200),
        (
            "an action pattern that matches by prefix",
            Some(allow(json!("b2bi:Get*"))),
            200,
        ),
        (
            "an action pattern that matches with ?",
            Some(allow(json!("b2bi:GetProfil?"))),
            200,
        ),
        (
            "only another action",
            Some(allow(json!("b2bi:ListProfiles"))),
            403,
        ),
        (
            "another service's actions",
            Some(allow(json!("s3:*"))),
            403,
        ),
        (
            "an Allow scoped to this very profile",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "b2bi:GetProfile",
                    "Resource": profile_arn.clone()
                }]
            })),
            200,
        ),
        (
            "an Allow scoped to every profile of the account",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "b2bi:GetProfile",
                    "Resource": format!("arn:aws:b2bi:*:{ACCOUNT}:profile/*")
                }]
            })),
            200,
        ),
        (
            "an Allow scoped to another profile",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "b2bi:GetProfile",
                    "Resource": format!("arn:aws:b2bi:us-west-2:{ACCOUNT}:profile/p-00000000000000000")
                }]
            })),
            403,
        ),
        (
            "a NotAction naming another action",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "NotAction": "b2bi:DeleteProfile",
                    "Resource": "*"
                }]
            })),
            200,
        ),
        (
            "a NotAction naming this one",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "NotAction": "b2bi:GetProfile",
                    "Resource": "*"
                }]
            })),
            403,
        ),
        (
            "a Deny beside an Allow, in either order",
            Some(json!({
                "Statement": [
                    { "Effect": "Deny", "Action": "b2bi:*", "Resource": "*" },
                    { "Effect": "Allow", "Action": "b2bi:GetProfile", "Resource": "*" }
                ]
            })),
            403,
        ),
        (
            "a single statement object rather than a list",
            Some(json!({
                "Statement": { "Effect": "Allow", "Action": "b2bi:GetProfile", "Resource": "*" }
            })),
            200,
        ),
        (
            // A statement this emulator cannot decide is applied in the direction that
            // cannot grant more than IAM would: an Allow carrying one grants nothing, so
            // the request falls to IAM's default deny.
            "an Allow carrying a Condition this emulator does not evaluate",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "b2bi:GetProfile",
                    "Resource": "*",
                    "Condition": { "StringEquals": { "aws:RequestedRegion": "us-west-2" } }
                }]
            })),
            403,
        ),
        (
            // …and a Deny carrying one still denies: the condition might hold, and a Deny
            // that is in force wins. The evaluation fails closed on both sides.
            "a Deny carrying a Condition, beside an Allow",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "b2bi:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "b2bi:GetProfile",
                        "Resource": "*",
                        "Condition": { "StringEquals": { "aws:RequestedRegion": "us-west-2" } }
                    }
                ]
            })),
            403,
        ),
        (
            // IAM reports a policy document percent-encoded, so one stored in that form
            // is read rather than skipped — a Deny must not be missed because of its
            // encoding.
            "a percent-encoded Deny",
            Some(json!(
                "%7B%22Statement%22%3A%5B%7B%22Effect%22%3A%22Deny%22%2C\
                 %22Action%22%3A%22b2bi%3A%2A%22%2C%22Resource%22%3A%22%2A%22%7D%5D%7D"
            )),
            403,
        ),
        (
            "a document that is not a policy at all",
            Some(json!("not a policy")),
            403,
        ),
    ];

    for (case, policy, expected) in cases {
        match &policy {
            Some(document) => emulator.given_role_policy(document),
            None => emulator.given_no_role_policy(),
        }
        let (status, response) = call_as(
            &emulator.router,
            SESSION_KEY_ID,
            "GetProfile",
            json!({ "profileId": profile_id.clone() }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if expected == 403 {
            assert_eq!(
                error_code(&response),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {response}"
            );
        } else {
            assert_eq!(
                response["profileId"],
                json!(profile_id),
                "case {case:?}: an authorized read must answer with the profile, got \
                 {response}"
            );
        }
    }
}

/// RULE: the decision is taken by EVERY implemented operation whose model declares
/// `AccessDeniedException` — not only by the one a report happened to name — so an explicit
/// `Deny` on that operation's own action refuses it with 403 while a policy without that
/// `Deny` lets the request through to its own answer.
///
/// The rows are every b2bi operation that models the error and is implemented, covering
/// both shapes the decision takes: an operation addressing a resource ARN and one
/// addressing none (a create, a list, a dry run). Each row's request body carries the
/// members its model marks required, so the call reaches the handler rather than being
/// refused by input validation; the permitted arm therefore asserts only that the answer is
/// NOT the authorization refusal, since what else a row answers is its own operation's
/// business (an absent S3 object, say).
#[tokio::test]
async fn every_operation_that_models_access_denied_takes_the_decision() {
    let emulator = Emulator::new();
    emulator.given_session();

    let x12_details = json!({ "transactionSet": "X12_850", "version": "VERSION_4010" });
    // Each row: the operation and a request body satisfying its modelled required members
    // (`smithy_query(query="operation",operation=...)`).
    let cases: Vec<(&str, Value)> = vec![
        ("CreateProfile", create_profile()),
        ("ListProfiles", json!({})),
        ("ListCapabilities", json!({})),
        ("ListPartnerships", json!({})),
        ("ListTransformers", json!({})),
        (
            "CreateTransformer",
            json!({
                "name": "authorization-table",
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": { "x12": x12_details.clone() }
                },
                "mapping": { "templateLanguage": "JSONATA", "template": "$" }
            }),
        ),
        (
            "TestMapping",
            json!({
                "inputFileContent": "{\"total\":1}",
                "mappingTemplate": "$",
                "fileFormat": "JSON"
            }),
        ),
        (
            "TestParsing",
            json!({
                "inputFile": { "bucketName": "edi-samples", "key": "sample.edi" },
                "fileFormat": "JSON",
                "ediType": { "x12Details": x12_details.clone() }
            }),
        ),
        (
            "CreateStarterMappingTemplate",
            json!({
                "mappingType": "JSONATA",
                "templateDetails": { "x12": x12_details.clone() }
            }),
        ),
        (
            "GenerateMapping",
            json!({
                "inputFileContent": "{\"total\":1}",
                "outputFileContent": "{\"total\":1}",
                "mappingType": "JSONATA"
            }),
        ),
    ];

    for (operation, body) in cases {
        // Denied: everything this service can do is allowed EXCEPT this one action.
        emulator.given_role_policy(&json!({
            "Statement": [
                { "Effect": "Allow", "Action": "b2bi:*", "Resource": "*" },
                {
                    "Effect": "Deny",
                    "Action": format!("b2bi:{operation}"),
                    "Resource": "*"
                }
            ]
        }));
        let (status, refused) =
            call_as(&emulator.router, SESSION_KEY_ID, operation, body.clone()).await;
        assert_eq!(
            status, 403,
            "case {operation:?}: an explicit Deny on b2bi:{operation} must refuse the call \
             with HTTP 403, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {operation:?}: expected an AccessDeniedException, got {refused}"
        );

        // Permitted: the same request with the Deny lifted is not refused by authorization.
        emulator.given_role_policy(&allow(json!("b2bi:*")));
        let (status, answered) = call_as(&emulator.router, SESSION_KEY_ID, operation, body).await;
        assert_ne!(
            status, 403,
            "case {operation:?}: with the Deny lifted the call must not be refused by \
             authorization, got {status} {answered}"
        );
        assert_ne!(
            error_code(&answered),
            "AccessDeniedException",
            "case {operation:?}: with the Deny lifted the answer must not be an \
             AccessDeniedException, got {answered}"
        );
    }
}

/// RULE: a principal whose grant this emulator cannot READ is served rather than refused —
/// an AWS-managed policy's document belongs to AWS, not to the account, so the IAM store
/// holds none and refusing on evidence it does not have would deny access AWS permits —
/// while an attachment naming a CUSTOMER-managed policy the account does not hold grants
/// nothing, because the account is the authority on its own policies. An explicit `Deny`
/// in a document this emulator CAN read still refuses either way.
#[tokio::test]
async fn an_attachment_this_emulator_cannot_read_is_served_rather_than_refused() {
    let emulator = Emulator::new();
    emulator.given_session();
    emulator.given_no_role_policy();

    // Each row: the case, the policy ARN attached to the role, the inline policy beside it
    // (none when `None`), and the expected status of CreateProfile.
    let customer_managed = format!("arn:aws:iam::{ACCOUNT}:policy/b2bi-access");
    let cases: Vec<(&str, &str, Option<Value>, u16)> = vec![
        (
            "a customer-managed policy the account does not hold",
            customer_managed.as_str(),
            None,
            403,
        ),
        (
            "an AWS-managed policy, whose document AWS owns",
            "arn:aws:iam::aws:policy/AmazonB2BIFullAccess",
            None,
            200,
        ),
        (
            "…and an explicit Deny this emulator can read still refuses",
            "arn:aws:iam::aws:policy/AmazonB2BIFullAccess",
            Some(json!({
                "Statement": [{ "Effect": "Deny", "Action": "b2bi:*", "Resource": "*" }]
            })),
            403,
        ),
    ];

    for (case, arn, inline, expected) in cases {
        emulator.given_attached_policy(arn);
        match &inline {
            Some(document) => emulator.given_role_policy(document),
            None => emulator.given_no_role_policy(),
        }
        let (status, response) = call_as(
            &emulator.router,
            SESSION_KEY_ID,
            "CreateProfile",
            create_profile(),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if expected == 403 {
            assert_eq!(
                error_code(&response),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {response}"
            );
        }
    }
}

/// RULE: a request whose access key id names no session speaks for the ambient identity —
/// the long-term key this emulator publishes, the account's own administrator — so it is
/// served whatever policies the account's roles carry.
#[tokio::test]
async fn a_request_that_names_no_session_is_served_as_the_ambient_identity() {
    let emulator = Emulator::new();
    emulator.given_session();
    // Everything this service can do is denied to the ROLE…
    emulator.given_role_policy(&json!({
        "Statement": [{ "Effect": "Deny", "Action": "b2bi:*", "Resource": "*" }]
    }));

    // …which says nothing about a request that does not speak for that role.
    let (status, created) = call(&emulator.router, "CreateProfile", create_profile()).await;
    assert_eq!(
        status, 200,
        "the ambient identity must be served, got {status} {created}"
    );

    // And the session's own request is refused, so the two really are different callers.
    let (status, refused) = call_as(
        &emulator.router,
        SESSION_KEY_ID,
        "CreateProfile",
        create_profile(),
    )
    .await;
    assert_eq!(
        status, 403,
        "the denied role must be refused, got {status} {refused}"
    );
}
