// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONDITIONS and the resource-based policy a Secrets Manager
//! authorization decision is made under, driven over the real wire path (an awsJson1_1
//! request through the multi-service router into the mounted service), so what is asserted
//! is what a client holding those credentials sees.
//!
//! THE RULE, in two halves:
//!
//! 1. Every operation's request, and the secret it addresses, populate the IAM condition
//!    keys this service documents for that operation — the tags the request sends, the tags
//!    the addressed secret carries, and each request member a condition key names — and a
//!    statement's `Condition` is decided against them. So a statement applies only while its
//!    condition HOLDS, whichever effect it states: a conditioned `Allow` admits exactly the
//!    matching request, and a conditioned `Deny` refuses exactly the request values it names
//!    — a key the request carries no value for cannot satisfy a positive comparison, so it
//!    cannot fire such a `Deny`.
//! 2. The decision is taken on the UNION of the caller's identity-based policies and the
//!    addressed secret's own resource-based policy: an `Allow` in either is a grant, so a
//!    principal holding no identity-based permission at all is served a secret whose policy
//!    names it, and an explicit `Deny` in either refuses. A statement naming the ACCOUNT
//!    delegates to that account's identity-based policies rather than granting on its own.
//!
//! A refusal is the AWS common `AccessDeniedException` at this service's client-error status,
//! carrying IAM's own sentence — which of its rules refused, and the document that refused.
//!
//! The tables span what the rule generalises over: the condition key (the request-tag
//! family, the resource-tag family, the tag-key list, the staging label, the
//! public-policy flag, the recovery window and the `SecretId`), the operator (`StringEquals`,
//! `StringLike`, `Bool`, the numeric comparisons, under a set qualifier), the spelling of a
//! clause's value (quoted and unquoted), whether the request satisfies the condition, carries
//! another value or carries none at all, and — for the resource-policy half — the principal
//! form the document names, the action it scopes the grant to, and the effect each side
//! states.
//!
//! `tests/all/identity_based_authorization.rs` spans the UNCONDITIONED decision (which
//! principal, which action, which resource) and owns the harness these cases reuse.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, given_secret, given_session, role_arn,
    router_with_identity, session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS, ACCOUNT_KEY_ID,
    SECRET_VALUE, SESSION_KEY_ID,
};

/// Seconds in a day, for reading a `DeletionDate` back as the window it was scheduled with.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// A policy allowing `action` on every resource, qualified by `condition`.
fn allow_if(action: &str, condition: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": action,
            "Resource": "*",
            "Condition": condition,
        }]
    })
}

/// A resource-based policy with one statement: `effect` on `action` for `principal`.
fn resource_policy(effect: &str, principal: Value, action: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "SharedWithOnePrincipal",
            "Effect": effect,
            "Principal": {"AWS": principal},
            "Action": action,
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Create one secret with the account's own credential, which this gate serves, and return
/// the ARN it holds the secret under.
async fn given_secret_with(
    router: &MultiServiceRouter,
    body: Value,
    case: &str,
) -> String {
    let (status, created) = call_as(router, ACCOUNT_KEY_ID, "CreateSecret", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the account's own credential must be served the create, got {status} \
         {created}"
    );
    created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reports no ARN: {created}"))
        .to_string()
}

/// Assert `answer` is the refusal IAM words for `reason`, naming the session that signed,
/// `action` and `resource`.
fn assert_refused(case: &str, status: u16, answer: &Value, action: &str, resource: &str, reason: &str) {
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        ACCESS_DENIED,
        "case {case:?}: expected an {ACCESS_DENIED}, got {answer}"
    );
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
        session_arn()
    );
    assert_eq!(
        error_message(answer),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(answer)
    );
}

/// What a row of the conditioned-allow table asks for, and whether the condition it is
/// judged under holds for it.
struct ConditionRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The action the identity policy allows, and the operation the row calls.
    operation: &'static str,
    /// The `Condition` element the allow is qualified by.
    condition: Value,
    /// The request body, minus the members the row's setup fills in.
    body: Value,
    /// Whether the request satisfies that condition, so the call must be served.
    satisfies: bool,
}

/// THE RULE, half 1: a conditioned identity-policy `Allow` admits exactly the requests whose
/// own values satisfy its condition, and refuses every other — including a request that
/// carries no value for the key at all, which no positive comparison can hold of.
#[tokio::test]
async fn a_conditioned_allow_admits_exactly_the_requests_its_condition_names() {
    // The secret every row that addresses one reads, and the tags it carries: the
    // resource-tag keys are populated from THESE.
    let tagged = json!({
        "Name": "conditioned-tagged",
        "SecretString": SECRET_VALUE,
        "Tags": [{"Key": "team", "Value": "payments"}],
    });
    // A non-public document, so a `PutResourcePolicy` row is decided by the gate rather
        // than refused for what it would attach.
    let shareable = resource_policy(
        "Allow",
        json!("arn:aws:iam::210987654321:role/app-reader"),
        json!("secretsmanager:GetSecretValue"),
    );

    let rows = vec![
        ConditionRow {
            case: "a request tag the condition names",
            operation: "CreateSecret",
            condition: json!({"StringEquals": {"aws:RequestTag/owner": "platform"}}),
            body: json!({
                "Name": "conditioned-created",
                "SecretString": SECRET_VALUE,
                "Tags": [{"Key": "owner", "Value": "platform"}],
            }),
            satisfies: true,
        },
        ConditionRow {
            case: "a request tag carrying another value",
            operation: "CreateSecret",
            condition: json!({"StringEquals": {"aws:RequestTag/owner": "platform"}}),
            body: json!({
                "Name": "conditioned-created",
                "SecretString": SECRET_VALUE,
                "Tags": [{"Key": "owner", "Value": "someone-else"}],
            }),
            satisfies: false,
        },
        ConditionRow {
            case: "a create carrying no tag at all",
            operation: "CreateSecret",
            condition: json!({"StringEquals": {"aws:RequestTag/owner": "platform"}}),
            body: json!({ "Name": "conditioned-created", "SecretString": SECRET_VALUE }),
            satisfies: false,
        },
        ConditionRow {
            case: "the tag keys a create sends, under a set qualifier",
            operation: "CreateSecret",
            condition: json!({"ForAllValues:StringEquals": {"aws:TagKeys": ["owner"]}}),
            body: json!({
                "Name": "conditioned-created",
                "SecretString": SECRET_VALUE,
                "Tags": [{"Key": "owner", "Value": "platform"}],
            }),
            satisfies: true,
        },
        ConditionRow {
            case: "a tag key outside the ones the set qualifier admits",
            operation: "CreateSecret",
            condition: json!({"ForAllValues:StringEquals": {"aws:TagKeys": ["owner"]}}),
            body: json!({
                "Name": "conditioned-created",
                "SecretString": SECRET_VALUE,
                "Tags": [
                    {"Key": "owner", "Value": "platform"},
                    {"Key": "cost-centre", "Value": "17"},
                ],
            }),
            satisfies: false,
        },
        ConditionRow {
            case: "the addressed secret's own tag",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:ResourceTag/team": "payments"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: true,
        },
        ConditionRow {
            case: "the addressed secret's own tag, matched by a pattern",
            operation: "GetSecretValue",
            condition: json!({"StringLike": {"aws:ResourceTag/team": "pay*"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: true,
        },
        ConditionRow {
            case: "this service's own spelling of the resource-tag family",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"secretsmanager:ResourceTag/team": "payments"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: true,
        },
        ConditionRow {
            case: "a resource tag the addressed secret does not carry",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:ResourceTag/team": "platform"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: false,
        },
        ConditionRow {
            case: "a resource-tag condition against an UNtagged secret",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"aws:ResourceTag/team": "payments"}}),
            body: json!({ "SecretId": "conditioned-untagged" }),
            satisfies: false,
        },
        ConditionRow {
            case: "the staging label the read names",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"secretsmanager:VersionStage": "AWSCURRENT"}}),
            body: json!({ "SecretId": "conditioned-tagged", "VersionStage": "AWSCURRENT" }),
            satisfies: true,
        },
        ConditionRow {
            case: "another staging label of the same secret",
            operation: "GetSecretValue",
            condition: json!({"StringEquals": {"secretsmanager:VersionStage": "AWSCURRENT"}}),
            body: json!({ "SecretId": "conditioned-tagged", "VersionStage": "AWSPENDING" }),
            satisfies: false,
        },
        ConditionRow {
            case: "the public-policy flag the write states",
            operation: "PutResourcePolicy",
            condition: json!({"Bool": {"secretsmanager:BlockPublicPolicy": "true"}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "ResourcePolicy": shareable.clone(),
                "BlockPublicPolicy": true,
            }),
            satisfies: true,
        },
        ConditionRow {
            case: "the same clause with its value written unquoted",
            operation: "PutResourcePolicy",
            condition: json!({"Bool": {"secretsmanager:BlockPublicPolicy": true}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "ResourcePolicy": shareable.clone(),
                "BlockPublicPolicy": true,
            }),
            satisfies: true,
        },
        ConditionRow {
            case: "a write that turns the public-policy check off",
            operation: "PutResourcePolicy",
            condition: json!({"Bool": {"secretsmanager:BlockPublicPolicy": "true"}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "ResourcePolicy": shareable.clone(),
                "BlockPublicPolicy": false,
            }),
            satisfies: false,
        },
        ConditionRow {
            case: "a write that states no public-policy flag at all",
            operation: "PutResourcePolicy",
            condition: json!({"Bool": {"secretsmanager:BlockPublicPolicy": "true"}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "ResourcePolicy": shareable.clone(),
            }),
            satisfies: false,
        },
        ConditionRow {
            case: "a recovery window at the bound the numeric clause admits",
            operation: "DeleteSecret",
            condition: json!({
                "NumericGreaterThanEquals": {"secretsmanager:RecoveryWindowInDays": "30"}
            }),
            body: json!({ "SecretId": "conditioned-tagged", "RecoveryWindowInDays": 30 }),
            satisfies: true,
        },
        ConditionRow {
            case: "a recovery window below that bound",
            operation: "DeleteSecret",
            condition: json!({
                "NumericGreaterThanEquals": {"secretsmanager:RecoveryWindowInDays": "30"}
            }),
            body: json!({ "SecretId": "conditioned-tagged", "RecoveryWindowInDays": 7 }),
            satisfies: false,
        },
        ConditionRow {
            case: "the staging label a value write attaches",
            operation: "PutSecretValue",
            condition: json!({"StringEquals": {"secretsmanager:VersionStage": "AWSCURRENT"}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "SecretString": "a-rotated-credential",
                "VersionStages": ["AWSCURRENT"],
            }),
            satisfies: true,
        },
        ConditionRow {
            case: "a value write attaching another label",
            operation: "PutSecretValue",
            condition: json!({"StringEquals": {"secretsmanager:VersionStage": "AWSCURRENT"}}),
            body: json!({
                "SecretId": "conditioned-tagged",
                "SecretString": "a-rotated-credential",
                "VersionStages": ["AWSPENDING"],
            }),
            satisfies: false,
        },
        ConditionRow {
            case: "the SecretId the request addressed the secret by",
            operation: "DescribeSecret",
            condition: json!({"StringLike": {"secretsmanager:SecretId": "conditioned-*"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: true,
        },
        ConditionRow {
            case: "a SecretId clause the request's own value does not satisfy",
            operation: "DescribeSecret",
            condition: json!({"StringLike": {"secretsmanager:SecretId": "elsewhere-*"}}),
            body: json!({ "SecretId": "conditioned-tagged" }),
            satisfies: false,
        },
        ConditionRow {
            case: "the tag keys an untagging names",
            operation: "UntagResource",
            condition: json!({"ForAnyValue:StringEquals": {"aws:TagKeys": ["team"]}}),
            body: json!({ "SecretId": "conditioned-tagged", "TagKeys": ["team"] }),
            satisfies: true,
        },
        ConditionRow {
            case: "an untagging of another key",
            operation: "UntagResource",
            condition: json!({"ForAnyValue:StringEquals": {"aws:TagKeys": ["team"]}}),
            body: json!({ "SecretId": "conditioned-tagged", "TagKeys": ["owner"] }),
            satisfies: false,
        },
    ];

    for row in rows {
        let ConditionRow {
            case,
            operation,
            condition,
            body,
            satisfies,
        } = row;
        // A fresh store per row: no row can be served by another row's write, and an
        // allowed mutation cannot change what a later row addresses.
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let arn = given_secret_with(&router, tagged.clone(), case).await;
        let untagged = given_secret(&router, "conditioned-untagged").await;
        // A second version under another staging label, so a read scoped to one label is
        // refused by the DECISION rather than by there being no such version.
        let (status, staged) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "PutSecretValue",
            &json!({
                "SecretId": "conditioned-tagged",
                "SecretString": "the-pending-credential",
                "VersionStages": ["AWSPENDING"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the pending version must be staked, got {status} {staged}"
        );
        let action = format!("secretsmanager:{operation}");
        given_role_policy(&engine, Some(&allow_if(&action, condition)));

        let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, &body).await;
        if satisfies {
            assert_eq!(
                status, 200,
                "case {case:?}: the condition holds for this request, so {operation} must be \
                 served, got {status} {answer}"
            );
        } else {
            // What the refusal names: the ARN of the secret the request addressed, or —
            // for a create, whose secret was never made — the NAME it carried.
            let addressed = match (body["SecretId"].as_str(), body["Name"].as_str()) {
                (Some("conditioned-untagged"), _) => untagged.clone(),
                (Some(_), _) => arn.clone(),
                (None, Some(name)) => name.to_string(),
                (None, None) => panic!("case {case:?}: the row addresses nothing"),
            };
            let reason = format!("because no identity-based policy allows the {action} action");
            assert_refused(case, status, &answer, &action, &addressed, &reason);
        }
    }
}

/// What a row of the guardrail table asks of `DeleteSecret`, and what the guardrail's two
/// conditioned `Deny` statements make of it.
struct GuardrailRow {
    case: &'static str,
    /// The members the delete states beside its `SecretId`.
    members: Value,
    /// The recovery window the delete must be scheduled with, or `None` where the request
    /// is refused.
    window: Option<f64>,
}

/// THE RULE, half 1 for a `Deny`: a conditioned `Deny` refuses exactly the request values
/// its condition names. A request that carries NO value for the key it names cannot satisfy
/// that condition, so it is not refused — which is what leaves the compliant delete working
/// under the standard "allow deletes, forbid the irreversible ones" guardrail.
#[tokio::test]
async fn a_conditioned_deny_refuses_only_the_request_values_it_names() {
    // The guardrail: delete and describe allowed, a forced delete denied, and a window
    // shorter than the longest one denied.
    let guardrail = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["secretsmanager:DeleteSecret", "secretsmanager:DescribeSecret"],
                "Resource": "*",
            },
            {
                "Effect": "Deny",
                "Action": "secretsmanager:DeleteSecret",
                "Resource": "*",
                "Condition": {
                    "Bool": {"secretsmanager:ForceDeleteWithoutRecovery": "true"}
                },
            },
            {
                "Effect": "Deny",
                "Action": "secretsmanager:DeleteSecret",
                "Resource": "*",
                "Condition": {
                    "NumericLessThan": {"secretsmanager:RecoveryWindowInDays": "30"}
                },
            },
        ]
    });
    // The rows differ along the members the delete states: the compliant window, the
    // window neither Deny names, a window one Deny names, the flag the other names, and
    // the flag written false.
    let rows = [
        GuardrailRow {
            case: "the longest recovery window, which neither Deny covers",
            members: json!({ "RecoveryWindowInDays": 30 }),
            window: Some(30.0),
        },
        GuardrailRow {
            case: "a delete stating neither key, which neither Deny can fire on",
            members: json!({}),
            // "If you don't specify this value, it defaults to 30 days."
            window: Some(30.0),
        },
        GuardrailRow {
            case: "a forced delete the first Deny names",
            members: json!({ "ForceDeleteWithoutRecovery": true }),
            window: None,
        },
        GuardrailRow {
            case: "a window below the one the second Deny names",
            members: json!({ "RecoveryWindowInDays": 7 }),
            window: None,
        },
        GuardrailRow {
            case: "the force flag written false, which the first Deny does not cover",
            members: json!({ "ForceDeleteWithoutRecovery": false }),
            window: Some(30.0),
        },
    ];

    for GuardrailRow {
        case,
        members,
        window,
    } in rows
    {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let arn = given_secret(&router, "guarded-secret").await;
        given_role_policy(&engine, Some(&guardrail));

        let mut body = json!({ "SecretId": "guarded-secret" });
        for (member, value) in members.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
        let called_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs_f64();
        let (status, answer) = call_as(&router, SESSION_KEY_ID, "DeleteSecret", &body).await;

        let Some(days) = window else {
            assert_refused(
                case,
                status,
                &answer,
                "secretsmanager:DeleteSecret",
                &arn,
                "with an explicit deny in an identity-based policy",
            );
            // And the refused delete scheduled nothing.
            let (status, described) = call_as(
                &router,
                ACCOUNT_KEY_ID,
                "DescribeSecret",
                &json!({ "SecretId": "guarded-secret" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the owner's own read must be served, got {status} {described}"
            );
            assert!(
                described["DeletedDate"].is_null(),
                "case {case:?}: a refused delete must schedule nothing, got {described}"
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {case:?}: the guardrail permits this delete, got {status} {answer}"
        );
        let scheduled = answer["DeletionDate"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: the delete reports no DeletionDate: {answer}"));
        let elapsed_days = (scheduled - called_at) / SECONDS_PER_DAY;
        assert!(
            (days - 1.0..=days + 1.0).contains(&elapsed_days),
            "case {case:?}: expected a DeletionDate about {days} days out, got {elapsed_days} \
             days ({answer})"
        );
        // Read back through the describe the same role is allowed: the secret the delete
        // scheduled reports the date it was scheduled for.
        let (status, described) = call_as(
            &router,
            SESSION_KEY_ID,
            "DescribeSecret",
            &json!({ "SecretId": "guarded-secret" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe the guardrail allows must be served, got {status} \
             {described}"
        );
        assert_eq!(
            described["DeletedDate"].as_f64(),
            Some(scheduled),
            "case {case:?}: expected the describe to report the scheduled deletion, got \
             {described}"
        );
    }
}

/// What a row of the resource-policy table attaches, and what the named role may then do.
struct SharingRow {
    case: &'static str,
    /// The document attached to the secret, or `None` for the row that attaches nothing.
    document: Option<String>,
    /// The identity-based policy in force on the role, or `None` where it holds none.
    identity: Option<Value>,
    /// The operation the role calls, and whether it must be served.
    operation: &'static str,
    served: bool,
    /// Whether the SECRET'S OWNER, signing with the account's own credential, is still
    /// served its own read: a document that denies the whole ACCOUNT covers the owner too.
    owner_served: bool,
    /// Which of IAM's refusals the row expects, when it is refused.
    reason: &'static str,
}

/// THE RULE, half 2: the decision is the union of the two kinds of document. A secret's own
/// resource-based policy `Allow` naming the caller is a grant on its own — the role holds no
/// identity-based policy at all — it grants only the action it names, an explicit `Deny` in
/// either document refuses, and a statement naming the ACCOUNT delegates rather than grants.
#[tokio::test]
async fn a_resource_policy_decides_the_call_beside_the_identity_policies() {
    let account = ::engine::emulator_account_id();
    let allow_read_for = |principal: Value| {
        Some(resource_policy(
            "Allow",
            principal,
            json!("secretsmanager:GetSecretValue"),
        ))
    };
    let deny_identity = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*",
        }]
    });
    let allow_identity = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "secretsmanager:*",
            "Resource": "*",
        }]
    });

    let rows = vec![
        SharingRow {
            case: "no document attached at all",
            document: None,
            identity: None,
            operation: "GetSecretValue",
            served: false,
            owner_served: true,
            reason: "because no identity-based policy allows the secretsmanager:GetSecretValue \
                     action",
        },
        SharingRow {
            case: "an allow naming the role behind the session",
            document: allow_read_for(json!(role_arn())),
            identity: None,
            operation: "GetSecretValue",
            served: true,
            owner_served: true,
            reason: "",
        },
        SharingRow {
            case: "an allow naming the session itself",
            document: allow_read_for(json!(session_arn())),
            identity: None,
            operation: "GetSecretValue",
            served: true,
            owner_served: true,
            reason: "",
        },
        SharingRow {
            case: "an allow listing the role beside another principal",
            document: allow_read_for(json!([
                role_arn(),
                "arn:aws:iam::210987654321:role/app-reader",
            ])),
            identity: None,
            operation: "GetSecretValue",
            served: true,
            owner_served: true,
            reason: "",
        },
        SharingRow {
            case: "an allow naming another principal only",
            document: allow_read_for(json!("arn:aws:iam::210987654321:role/app-reader")),
            identity: None,
            operation: "GetSecretValue",
            served: false,
            owner_served: true,
            reason: "because no identity-based policy allows the secretsmanager:GetSecretValue \
                     action",
        },
        SharingRow {
            case: "an allow naming the account root, which delegates rather than grants",
            document: allow_read_for(json!(format!("arn:aws:iam::{account}:root"))),
            identity: None,
            operation: "GetSecretValue",
            served: false,
            owner_served: true,
            reason: "because no identity-based policy allows the secretsmanager:GetSecretValue \
                     action",
        },
        SharingRow {
            case: "an allow of the read does not cover another action",
            document: allow_read_for(json!(role_arn())),
            identity: None,
            operation: "DescribeSecret",
            served: false,
            owner_served: true,
            reason: "because no identity-based policy allows the secretsmanager:DescribeSecret \
                     action",
        },
        SharingRow {
            case: "an allow of every action of this service covers the describe",
            document: Some(resource_policy(
                "Allow",
                json!(role_arn()),
                json!("secretsmanager:*"),
            )),
            identity: None,
            operation: "DescribeSecret",
            served: true,
            owner_served: true,
            reason: "",
        },
        SharingRow {
            case: "an identity deny overrides the resource allow",
            document: allow_read_for(json!(role_arn())),
            identity: Some(deny_identity.clone()),
            operation: "GetSecretValue",
            served: false,
            owner_served: true,
            reason: "with an explicit deny in an identity-based policy",
        },
        SharingRow {
            case: "a resource deny overrides the identity allow",
            document: Some(resource_policy(
                "Deny",
                json!(role_arn()),
                json!("secretsmanager:GetSecretValue"),
            )),
            identity: Some(allow_identity.clone()),
            operation: "GetSecretValue",
            served: false,
            owner_served: true,
            reason: "with an explicit deny in a resource-based policy",
        },
        SharingRow {
            case: "a resource deny of the whole account covers each of its principals",
            document: Some(resource_policy(
                "Deny",
                json!(format!("arn:aws:iam::{account}:root")),
                json!("secretsmanager:GetSecretValue"),
            )),
            identity: Some(allow_identity.clone()),
            operation: "GetSecretValue",
            served: false,
            // The document denies the ACCOUNT, which is every principal of it: the owner's
            // own credential is refused by it too.
            owner_served: false,
            reason: "with an explicit deny in a resource-based policy",
        },
        SharingRow {
            case: "an identity allow stands on its own where the document names another \
                   principal",
            document: allow_read_for(json!("arn:aws:iam::210987654321:role/app-reader")),
            identity: Some(allow_identity.clone()),
            operation: "GetSecretValue",
            served: true,
            owner_served: true,
            reason: "",
        },
    ];

    for row in rows {
        let SharingRow {
            case,
            document,
            identity,
            operation,
            served,
            owner_served,
            reason,
        } = row;
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let arn = given_secret(&router, "shared-secret").await;
        given_role_policy(&engine, identity.as_ref());
        if let Some(document) = &document {
            let (status, attached) = call_as(
                &router,
                ACCOUNT_KEY_ID,
                "PutResourcePolicy",
                &json!({ "SecretId": "shared-secret", "ResourcePolicy": document }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the owner must be served the attach, got {status} {attached}"
            );
        }

        let (status, answer) =
            call_as(&router, SESSION_KEY_ID, operation, &json!({ "SecretId": &arn })).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: the union of the documents grants this call, got {status} \
                 {answer}"
            );
            if operation == "GetSecretValue" {
                assert_eq!(
                    answer["SecretString"],
                    json!(SECRET_VALUE),
                    "case {case:?}: the served read must carry the stored value, got {answer}"
                );
            }
        } else {
            assert_refused(
                case,
                status,
                &answer,
                &format!("secretsmanager:{operation}"),
                &arn,
                reason,
            );
            assert!(
                answer["SecretString"].is_null(),
                "case {case:?}: a refused read must serve no value, got {answer}"
            );
        }

        // Detaching the document returns the role to the refused state, and the owner's own
        // access is unaffected throughout.
        if document.is_some() && identity.is_none() {
            let (status, detached) = call_as(
                &router,
                ACCOUNT_KEY_ID,
                "DeleteResourcePolicy",
                &json!({ "SecretId": "shared-secret" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the owner must be served the detach, got {status} {detached}"
            );
            let (status, answer) =
                call_as(&router, SESSION_KEY_ID, operation, &json!({ "SecretId": &arn })).await;
            assert_refused(
                &format!("{case} (after the document is detached)"),
                status,
                &answer,
                &format!("secretsmanager:{operation}"),
                &arn,
                &format!(
                    "because no identity-based policy allows the secretsmanager:{operation} \
                     action"
                ),
            );
        }
        let (status, owned) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "GetSecretValue",
            &json!({ "SecretId": &arn }),
        )
        .await;
        let expected_owner_status = if owner_served {
            200
        } else {
            ACCESS_DENIED_STATUS
        };
        assert_eq!(
            status, expected_owner_status,
            "case {case:?}: expected the owner's own read to answer \
             {expected_owner_status}, got {status} {owned}"
        );
    }
}
