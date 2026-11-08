// SPDX-License-Identifier: CC0-1.0

//! Which DOCUMENTS a request to this service is judged on: every identity-based policy in
//! force on the calling principal, not only the inline ones.
//!
//! THE RULE: the permission of a request this service decides comes from the policy documents
//! in force on the calling principal — its inline policies AND the default version of every
//! managed policy ATTACHED to it, whether this account owns that policy or AWS publishes it.
//! The decision reads them through IAM's own collection of a principal's policies, so this
//! service sees exactly what `PutUserPolicy`, `CreatePolicy` and `AttachUserPolicy` put in
//! force; an attached policy that allows the action is a grant, and one that states nothing
//! about it leaves the request refused with AWS's own denial sentence.
//!
//! The rows span WHERE the caller's authority comes from: nothing at all, an inline policy, an
//! AWS-published managed policy that allows every action, an AWS-published one that states no
//! action of this service, and one this account created itself.
//!
//! The principals, their policies and their attachments are made with IAM's own operations
//! over the same engine, through the harness `tests/all/grant_authorization.rs` holds, so the
//! documents this service reads are the ones IAM wrote.

use serde_json::{json, Value};

use super::grant_authorization::{assert_denied, identity_allow, Emulator};
use super::harness::REGION;

/// The operation every row calls: one this service serves off the identity-based policies
/// alone, so what the row varies is the document in force and nothing else.
const OPERATION: &str = "ListAliases";

/// The action that operation is authorized under.
const ACTION: &str = "kms:ListAliases";

/// The resource an action carrying no resource type is judged against: every KMS resource of
/// this account in the Region the request was signed for.
fn service_wildcard() -> String {
    format!(
        "arn:aws:kms:{REGION}:{account}:*",
        account = engine::emulator_account_id()
    )
}

/// The sentence AWS refuses a request no identity-based policy allows with.
fn no_identity_policy(principal: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ACTION} on resource: {} \
         because no identity-based policy allows the {ACTION} action",
        service_wildcard()
    )
}

/// The ARN of one policy AWS publishes, read out of IAM's own catalog of those documents
/// (`service_com_amazonaws_iam_awsidentitymanagementv20100508::state::AWS_MANAGED_POLICIES`),
/// so a row attaches a policy IAM really publishes and the document in force is AWS's own.
fn aws_managed_policy_arn(policy_name: &str) -> String {
    service_com_amazonaws_iam_awsidentitymanagementv20100508::state::aws_managed_policy_named(
        policy_name,
    )
    .unwrap_or_else(|| panic!("IAM publishes the managed policy {policy_name:?}"))
    .arn()
}

/// The `Arn` of the policy a `CreatePolicy` answer reports, which is the identifier an attach
/// names it by — read out of the answer rather than rebuilt, so this suite encodes no ARN
/// layout of its own.
fn policy_arn(answered: &str, case: &str) -> String {
    let open = "<Arn>";
    let close = "</Arn>";
    let start = answered
        .find(open)
        .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy answered no Arn: {answered}"))
        + open.len();
    let end = answered[start..]
        .find(close)
        .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy's Arn is never closed: {answered}"))
        + start;
    answered[start..end].to_string()
}

/// Where a row's caller gets its authority from.
enum Authority {
    /// No policy at all.
    None,
    /// An inline policy allowing `kms:ListAliases`, which is the case the sibling suites
    /// already span — here as the control the attachments are read against.
    Inline,
    /// A policy AWS publishes, attached to the caller.
    AwsManaged(&'static str),
    /// A policy this ACCOUNT created, attached to the caller.
    AccountManaged,
}

/// One row: where the caller's authority comes from, and whether the call is served.
struct Row {
    case: &'static str,
    authority: Authority,
    served: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no policy at all",
            authority: Authority::None,
            served: false,
        },
        Row {
            case: "an inline policy allowing the action",
            authority: Authority::Inline,
            served: true,
        },
        Row {
            case: "an attached AWS-published policy that allows every action",
            authority: Authority::AwsManaged("AdministratorAccess"),
            served: true,
        },
        Row {
            // The attachment a Lambda execution role carries: its document states the
            // function's own log writes and nothing of this service.
            case: "an attached AWS-published policy that states no action of this service",
            authority: Authority::AwsManaged("AWSLambdaBasicExecutionRole"),
            served: false,
        },
        Row {
            case: "an attached policy this account created",
            authority: Authority::AccountManaged,
            served: true,
        },
    ]
}

/// RULE: the documents in force on the calling principal decide the request — its inline
/// policies and the default version of every policy attached to it, AWS's own published
/// policies included — so an attachment that allows the action is a grant and one that states
/// nothing about it leaves the request refused.
#[tokio::test]
async fn an_attached_policys_document_is_in_force_on_the_caller() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let user_name = format!("AttachedAuthority{index}");
        let inline = match row.authority {
            Authority::Inline => Some(identity_allow(json!(ACTION), "*")),
            _ => None,
        };
        let (key_id, user_arn) = emulator.iam_user(&user_name, inline.as_deref()).await;

        let attachment = match row.authority {
            Authority::AwsManaged(policy_name) => Some(aws_managed_policy_arn(policy_name)),
            Authority::AccountManaged => {
                let created = emulator
                    .query(
                        "iam",
                        "2010-05-08",
                        "CreatePolicy",
                        &[
                            ("PolicyName", format!("AttachedAuthority{index}Policy")),
                            ("PolicyDocument", identity_allow(json!(ACTION), "*")),
                        ],
                    )
                    .await;
                assert_eq!(
                    created.0, 200,
                    "case {case:?}: creating the account's own policy must succeed, got {} {}",
                    created.0, created.1
                );
                // The ARN the create ANSWERED with, which is the one the attach has to name:
                // the layout of a policy's ARN is the service's own and not this suite's.
                Some(policy_arn(&created.1, case))
            }
            Authority::None | Authority::Inline => None,
        };
        if let Some(policy_arn) = &attachment {
            let (status, body) = emulator
                .query(
                    "iam",
                    "2010-05-08",
                    "AttachUserPolicy",
                    &[
                        ("UserName", user_name.clone()),
                        ("PolicyArn", policy_arn.clone()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: attaching {policy_arn} must succeed, got {status} {body}"
            );
        }

        let (status, payload) = emulator.call(&key_id, OPERATION, &json!({"Limit": 1})).await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the document in force to grant {ACTION}, got {status} \
                 with {payload}"
            );
            assert!(
                payload.get("Aliases").is_some_and(Value::is_array),
                "case {case:?}: expected the operation's own answer, got {payload}"
            );
            continue;
        }
        assert_denied(case, status, &payload, &no_identity_policy(&user_arn));
    }
}
