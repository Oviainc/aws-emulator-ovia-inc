// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION a control-plane request that hands this
//! service a ROLE is subject to, driven over each service's own wire protocol through
//! the production `MultiServiceRouter` with Step Functions, IAM and KMS mounted on ONE
//! engine — the shape `core/router/src/lib.rs` mounts them in — so the policies a
//! decision reads are the documents IAM's own handlers wrote.
//!
//! The rule under test: naming a `roleArn` in a request HANDS this service a role, so
//! the request is performed only where the caller's identity-based policies allow it
//! the `iam:PassRole` for THAT role; otherwise it is refused with HTTP 400
//! `AccessDeniedException` and nothing is created or changed. The second half of the
//! same rule is about the role rather than the caller: Step Functions seals an
//! execution's data under the state machine's customer managed key AS that role, so a
//! start is refused with HTTP 400 `KmsAccessDeniedException` where the role's policies
//! do not allow it to mint a data key with that key.
//!
//! The rows differ along what the rule generalises over: the operation that hands the
//! role over (`CreateStateMachine`, `UpdateStateMachine`, `TestState`), what the
//! caller's documents say about the pass (an allow scoped to a path of roles, an
//! allow of every role, an explicit deny, documents that say nothing about
//! `iam:PassRole` at all), which role the request names (one the grant covers, one it
//! does not, one of ANOTHER account) and the kind of caller (a bounded IAM user, and
//! the account's own published credential, which no identity policy governs).
//!
//! The harness (`Chain`, `iam_user`, the key policies) is the one
//! `execution_data_encryption` builds for the same three services; `tests/all.rs` is
//! this binary's crate root, so it is reached through
//! `crate::execution_data_encryption`.

use serde_json::{json, Value};

use crate::execution_data_encryption::{
    customer_managed, granting_policy, Chain, SFN,
};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, HELLO_DEFINITION};

/// The code AWS refuses an authorization failure with, and the status it carries.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The code a start whose role may not use the state machine's key is refused with.
const KMS_ACCESS_DENIED: &str = "KmsAccessDeniedException";

/// The IAM action a request that hands this service a role performs.
const PASS_ROLE: &str = "iam:PassRole";

/// A role of this account, under the path a grant below is scoped to.
fn passable_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/sfn/Runner")
}

/// Another role of this account, OUTSIDE that path.
fn other_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/Other")
}

/// A role of ANOTHER account, which only that account can grant a pass of.
const CROSS_ACCOUNT_ROLE: &str = "arn:aws:iam::210987654321:role/sfn/Runner";

/// An identity policy allowing every Step Functions action and `iam:PassRole` only for
/// the roles `roles` names.
fn passing_only(roles: &[String]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "states:*", "Resource": "*" },
            { "Effect": "Allow", "Action": PASS_ROLE, "Resource": roles },
        ]
    })
}

/// An identity policy allowing every Step Functions action and saying nothing at all
/// about passing a role.
fn passing_nothing() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "states:*", "Resource": "*" }]
    })
}

/// An identity policy that allows passing every role and then DENIES it explicitly,
/// which is what an explicit deny winning over every allow looks like.
fn passing_denied() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": ["states:*", PASS_ROLE], "Resource": "*" },
            { "Effect": "Deny", "Action": PASS_ROLE, "Resource": "*" },
        ]
    })
}

/// What a request that hands a role over looks like for one operation, and what it
/// names the role in.
struct Handing {
    case: &'static str,
    operation: &'static str,
    /// The request, built from the role the caller tries to pass and from the state
    /// machine an update addresses.
    request: fn(&str, &str) -> Value,
}

/// Every operation of this service that hands it a role.
fn handings() -> Vec<Handing> {
    vec![
        Handing {
            case: "CreateStateMachine names the role every execution acts as",
            operation: "CreateStateMachine",
            request: |role, machine| {
                json!({
                    // The name is derived from the state machine this row also
                    // updates, so two rows never collide on one name.
                    "name": format!("{}-created", name_of(machine)),
                    "definition": HELLO_DEFINITION,
                    "roleArn": role,
                })
            },
        },
        Handing {
            case: "UpdateStateMachine replaces the role of a state machine",
            operation: "UpdateStateMachine",
            request: |role, machine| {
                json!({ "stateMachineArn": machine, "roleArn": role })
            },
        },
        Handing {
            case: "TestState runs a state as the role",
            operation: "TestState",
            request: |role, _machine| {
                json!({
                    "definition": r#"{"Type":"Pass","End":true}"#,
                    "roleArn": role,
                })
            },
        },
    ]
}

/// The name part of a state machine ARN, which a row builds a fresh name from.
fn name_of(state_machine_arn: &str) -> &str {
    state_machine_arn
        .rsplit(':')
        .next()
        .expect("a state machine ARN carries a name")
}

/// A caller's permissions, and the roles it may hand over under them.
struct Caller {
    case: &'static str,
    /// The inline policy the caller's IAM user carries, or `None` for the account's
    /// own published credential, which holds no identity policy at all.
    policy: Option<Value>,
    /// Whether a pass of [`passable_role`] is allowed.
    passes_the_scoped_role: bool,
    /// Whether a pass of [`other_role`] is allowed.
    passes_another_role: bool,
    /// Whether a pass of [`CROSS_ACCOUNT_ROLE`] is allowed.
    passes_a_cross_account_role: bool,
}

/// Every caller the rows below are taken as.
fn callers() -> Vec<Caller> {
    vec![
        Caller {
            case: "a user whose grant is scoped to one path of roles",
            policy: Some(passing_only(&[format!("arn:aws:iam::{ACCOUNT}:role/sfn/*")])),
            passes_the_scoped_role: true,
            passes_another_role: false,
            // `iam:PassRole` is granted by the account that HOLDS the role, and this
            // account's grant names its own roles alone.
            passes_a_cross_account_role: false,
        },
        Caller {
            case: "a user whose grant names every role",
            policy: Some(passing_only(&["*".to_string()])),
            passes_the_scoped_role: true,
            passes_another_role: true,
            passes_a_cross_account_role: true,
        },
        Caller {
            case: "a user whose documents say nothing about passing a role",
            policy: Some(passing_nothing()),
            passes_the_scoped_role: false,
            passes_another_role: false,
            passes_a_cross_account_role: false,
        },
        Caller {
            case: "a user whose documents deny the pass explicitly",
            policy: Some(passing_denied()),
            passes_the_scoped_role: false,
            passes_another_role: false,
            passes_a_cross_account_role: false,
        },
        Caller {
            case: "the account's own published credential",
            policy: None,
            passes_the_scoped_role: true,
            passes_another_role: true,
            passes_a_cross_account_role: true,
        },
    ]
}

/// A request that hands this service a role is served only where the caller's
/// identity-based policies allow the `iam:PassRole` for that role, and a refused one
/// changes nothing.
#[tokio::test]
async fn a_request_handing_over_a_role_needs_the_callers_pass_role() {
    for (index, caller) in callers().into_iter().enumerate() {
        let chain = Chain::new();
        // The caller: a bounded IAM user, or the account's own credential.
        let (key, principal) = match &caller.policy {
            Some(policy) => {
                let (key, arn) = chain
                    .iam_user(&format!("passer-{index}"), policy)
                    .await;
                (key, arn)
            }
            None => (
                "AKIDEXAMPLE".to_string(),
                format!("arn:aws:iam::{ACCOUNT}:root"),
            ),
        };

        for (position, handing) in handings().into_iter().enumerate() {
            // Each row gets a state machine of its own, created with the account's own
            // credential so the row tests the handing and not the setup.
            let created = chain
                .sfn(
                    "CreateStateMachine",
                    &json!({
                        "name": format!("pass-role-{index}-{position}"),
                        "definition": HELLO_DEFINITION,
                        "roleArn": passable_role(),
                    }),
                )
                .await;
            let machine = string(&created, "stateMachineArn");

            for (role, allowed) in [
                (passable_role(), caller.passes_the_scoped_role),
                (other_role(), caller.passes_another_role),
                (
                    CROSS_ACCOUNT_ROLE.to_string(),
                    caller.passes_a_cross_account_role,
                ),
            ] {
                let case = format!("{} / {} / {role}", caller.case, handing.case);
                let body = (handing.request)(&role, &machine);
                let (status, answer) = chain.call_as(&key, &SFN, handing.operation, &body).await;
                if allowed {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a pass the caller's policies allow is served, got \
                         {status} with {answer:?}"
                    );
                    continue;
                }
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case:?}: a pass the caller's policies do not allow is refused, got \
                     {status} with {answer:?}"
                );
                let message = answer
                    .get("message")
                    .or_else(|| answer.get("Message"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                for named in [principal.as_str(), PASS_ROLE, role.as_str()] {
                    assert!(
                        message.contains(named),
                        "case {case:?}: expected the refusal to name {named:?}, got \
                         {message:?}"
                    );
                }
                // Nothing was created or changed: the state machine still runs the
                // role it was created with, and no state machine was minted under the
                // name a refused create named.
                let described = chain
                    .sfn("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
                    .await;
                assert_eq!(
                    string(&described, "roleArn"),
                    passable_role(),
                    "case {case:?}: a refused update leaves the role it could not pass \
                     unapplied, got {described:?}"
                );
            }
        }
    }
}

/// A role carrying `policy` inline and trusting this service, by its own ARN.
async fn role_allowing(chain: &Chain, name: &str, policy: &Value) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "states.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }]
    });
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust.to_string()),
            ],
        )
        .await;
    let arn = crate::execution_data_encryption::xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
    chain
        .iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    arn
}

/// An identity policy for an execution role: every Step Functions action, and the KMS
/// actions `actions` names.
fn role_policy(actions: &[&str]) -> Value {
    let mut statements = vec![json!({
        "Effect": "Allow",
        "Action": "states:*",
        "Resource": "*"
    })];
    if !actions.is_empty() {
        statements.push(json!({
            "Effect": "Allow",
            "Action": actions,
            "Resource": "*"
        }));
    }
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// Starting an execution of a state machine on a customer managed key needs the
/// EXECUTION ROLE's permission to mint a data key with that key: the start is refused
/// with `KmsAccessDeniedException` while the role's policies do not allow it, and
/// served as soon as they do.
///
/// The rows differ along what the role's documents say about the KMS call the sealing
/// makes (nothing, the call itself, every KMS action, an explicit deny) and along
/// whether IAM holds the role at all — a role of this account the store has no record
/// for is evidence of nothing, so the start is served.
#[tokio::test]
async fn a_start_needs_the_execution_roles_data_key_permission() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;

    // (case, the role's policy — `None` for a role IAM does not hold, started?)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        (
            "a role whose documents say nothing about the KMS call",
            Some(role_policy(&[])),
            false,
        ),
        (
            "a role allowed the KMS call the sealing makes",
            Some(role_policy(&["kms:GenerateDataKey"])),
            true,
        ),
        (
            "a role allowed every KMS action",
            Some(role_policy(&["kms:*"])),
            true,
        ),
        (
            "a role allowed the call and then denied it",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "kms:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "kms:GenerateDataKey", "Resource": "*" },
                ]
            })),
            false,
        ),
        // A role of this account IAM holds no record for says nothing about what it
        // may do, which is evidence of nothing: the start is served.
        ("a role IAM holds no record for", None, true),
    ];

    for (index, (case, policy, started)) in rows.into_iter().enumerate() {
        let role = match &policy {
            Some(policy) => role_allowing(&chain, &format!("key-user-{index}"), policy).await,
            None => format!("arn:aws:iam::{ACCOUNT}:role/not-held-{index}"),
        };
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("role-key-{index}"),
                    "definition": HELLO_DEFINITION,
                    "roleArn": role,
                    "encryptionConfiguration": customer_managed(&key),
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");

        // Every attempt is made TWICE: a decision taken against the documents in force
        // is the same decision on the next call, so nothing of it latches.
        for attempt in 1..=2 {
            let (status, answer) = chain
                .call_as(
                    "AKIDEXAMPLE",
                    &SFN,
                    "StartExecution",
                    &json!({
                        "stateMachineArn": machine,
                        "name": format!("run-{attempt}"),
                        "input": r#"{"n":1}"#,
                    }),
                )
                .await;
            if started {
                assert_eq!(
                    status, 200,
                    "case {case:?} (attempt {attempt}): a role allowed to seal the data starts \
                     the execution, got {status} with {answer:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
                    "case {case:?} (attempt {attempt}): a role that may not mint a data key \
                     cannot start the execution, got {status} with {answer:?}"
                );
            }
        }

        // A refused start started nothing: the state machine has the executions the
        // served rows started and none of the refused ones.
        let listed = chain
            .sfn("ListExecutions", &json!({ "stateMachineArn": machine }))
            .await;
        let count = listed
            .get("executions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let expected = if started { 2 } else { 0 };
        assert_eq!(
            count, expected,
            "case {case:?}: expected {expected} execution(s) to have been started, got {listed:?}"
        );
    }
}
