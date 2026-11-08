// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS file-system-policy slice: `PutFileSystemPolicy` and
//! `DescribeFileSystemPolicy`.
//!
//! The rules these assert, over a SET of documents rather than on one recorded call:
//!
//! * a `FileSystemPolicy` is a document a file system CARRIES — a file system none was
//!   ever stored on answers the read with 404 `PolicyNotFound` and no sentence;
//! * a set stores the submitted document, in the one canonical layout every AWS API that
//!   takes a policy reports a policy in, and the read observes it;
//! * a document that is not an IAM resource-based policy is refused 400
//!   `InvalidPolicyException`, and so is one whose explicit `Deny` would leave the calling
//!   principal unable to make a future `PutFileSystemPolicy` request on this file system —
//!   the POLICY LOCKOUT SAFETY CHECK — unless the request sets
//!   `BypassPolicyLockoutSafetyCheck`;
//! * a delete returns the file system to the default policy, which is where one that never
//!   carried an explicit policy already is, so it is idempotent;
//! * every refusal stores nothing.
//!
//! The tables vary what the rules generalise over: which element of the policy grammar a
//! document breaks, how the `Deny` reaches the caller (every principal, the account root,
//! the bare account id, an action wildcard, a `NotAction`, an omitted `Resource`), which
//! statements leave the path open (another action, another resource, another account, an
//! `Allow`), and the form of the `FileSystemId`.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::harness::{settle, Emulator, Reply};

/// The sentence the policy-lockout safety check refuses a request with, as AWS words it:
/// fixed text that names the member bypassing the check and interpolates nothing from the
/// request.
const LOCKOUT_MESSAGE: &str =
    "This policy would prevent future FileSystemPolicy updates. Retry the request with the \
     BypassPolicyLockoutSafetyCheck parameter to apply this policy.";

/// The IAM action a file system's own policy can withhold — the one that replaces that
/// policy.
const POLICY_MANAGEMENT_ACTION: &str = "elasticfilesystem:PutFileSystemPolicy";

/// One policy document around `statement`.
fn policy(statement: Value) -> Value {
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// The account segment of an ARN: `arn:<partition>:<service>:<region>:<account>:<rest>`.
fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// The whole `PolicyNotFound` refusal: the modelled status and error shape, and NO
/// sentence — present in the error document as JSON `null`.
fn assert_policy_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("PolicyNotFound"),
        "case {case:?}: expected the modelled PolicyNotFound, got {:?}",
        reply.error_type
    );
    assert!(
        reply.get("Message").is_null(),
        "case {case:?}: the refusal carries no sentence, got {:?}",
        reply.get("Message")
    );
}

/// The whole `InvalidPolicyException` refusal: "Returned if the FileSystemPolicy is
/// malformed or contains an error ... Returned in the case of a policy lockout safety
/// check error."
fn assert_invalid_policy(case: &str, reply: &Reply, expected_message: Option<&str>) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("InvalidPolicyException"),
        "case {case:?}: expected the modelled InvalidPolicyException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("ErrorCode"),
        "InvalidPolicyException",
        "case {case:?}: unexpected ErrorCode, got {:?}",
        reply.get("ErrorCode")
    );
    let message = reply.string("Message");
    assert!(
        message.ends_with('.'),
        "case {case:?}: the sentence is terminated with a period, got {message:?}"
    );
    if let Some(expected) = expected_message {
        assert_eq!(
            message, expected,
            "case {case:?}: unexpected Message, got {message:?}"
        );
    }
}

/// One row of the lockout table: the case, and the document BUILDER it applies — a
/// builder rather than a document, because each row is applied to its own file system and
/// names that file system's ARN.
type LockoutRow = (&'static str, fn(&str) -> Value);

/// The documents whose `Deny` reaches the calling principal for
/// [`POLICY_MANAGEMENT_ACTION`] on the file system each is built against, so the lockout
/// safety check refuses each of them. Every row denies through a DIFFERENT spelling of the
/// same rule.
fn lockout_documents() -> Vec<LockoutRow> {
    vec![
        (
            "denies the action to every principal on this file system",
            |arn| {
                policy(json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": POLICY_MANAGEMENT_ACTION,
                    "Resource": arn,
                }))
            },
        ),
        (
            "denies every action of this service on every resource",
            |_| {
                policy(json!({
                    "Effect": "Deny",
                    "Principal": { "AWS": "*" },
                    "Action": "elasticfilesystem:*",
                    "Resource": "*",
                }))
            },
        ),
        ("denies the action through a wildcard", |arn| {
            policy(json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "elasticfilesystem:Put*",
                "Resource": arn,
            }))
        }),
        (
            "denies the caller's account root among several actions",
            |arn| {
                policy(json!({
                    "Effect": "Deny",
                    "Principal": { "AWS": format!("arn:aws:iam::{}:root", account_of(arn)) },
                    "Action": ["elasticfilesystem:ClientMount", POLICY_MANAGEMENT_ACTION],
                    "Resource": arn,
                }))
            },
        ),
        (
            "denies the caller's account named by its bare id",
            |arn| {
                policy(json!({
                    "Effect": "Deny",
                    "Principal": { "AWS": account_of(arn) },
                    "Action": "*",
                    "Resource": arn,
                }))
            },
        ),
        ("denies everything except one client action", |arn| {
            policy(json!({
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": "elasticfilesystem:ClientMount",
                "Resource": arn,
            }))
        }),
        (
            "denies the action with no Resource element, which covers the file system the \
             policy is attached to",
            |_| {
                policy(json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": POLICY_MANAGEMENT_ACTION,
                }))
            },
        ),
        (
            "denies the action beside an Allow of it, which an explicit Deny overrides",
            |arn| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": "*",
                            "Action": POLICY_MANAGEMENT_ACTION,
                            "Resource": arn,
                        },
                        {
                            "Effect": "Deny",
                            "Principal": "*",
                            "Action": POLICY_MANAGEMENT_ACTION,
                            "Resource": arn,
                        },
                    ],
                })
            },
        ),
    ]
}

/// The documents that leave the calling principal a path to a future
/// [`POLICY_MANAGEMENT_ACTION`] on the file system `arn`, so the lockout safety check
/// accepts each of them without a bypass.
fn accepted_documents(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "allows mounting to every principal and denies nothing",
            policy(json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": ["elasticfilesystem:ClientMount", "elasticfilesystem:ClientWrite"],
                "Resource": arn,
            })),
        ),
        (
            "denies another action of this service",
            policy(json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "elasticfilesystem:ClientWrite",
                "Resource": arn,
            })),
        ),
        (
            "denies the action to a principal in another account",
            policy(json!({
                "Effect": "Deny",
                "Principal": { "AWS": "arn:aws:iam::210987654321:root" },
                "Action": POLICY_MANAGEMENT_ACTION,
                "Resource": arn,
            })),
        ),
        (
            "denies the action on another file system",
            policy(json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": POLICY_MANAGEMENT_ACTION,
                "Resource": format!(
                    "arn:aws:elasticfilesystem:us-east-1:{}:file-system/fs-0123456789abcdef",
                    account_of(arn)
                ),
            })),
        ),
        (
            "allows the action, which keeps the path open",
            policy(json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": POLICY_MANAGEMENT_ACTION,
                "Resource": arn,
            })),
        ),
        (
            "says nothing about the action at all",
            policy(json!({
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{}:root", account_of(arn)) },
                "Action": "elasticfilesystem:ClientRootAccess",
                "Resource": "*",
            })),
        ),
    ]
}

#[tokio::test]
async fn a_stored_policy_round_trips_and_its_rendering_is_a_fixed_point() {
    let efs = Emulator::new();
    let created = efs
        .create_file_system(json!({ "CreationToken": "policy-round-trip" }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status, created.body
    );
    let file_system_id = created.string("FileSystemId");
    let file_system_arn = created.string("FileSystemArn");
    settle().await;

    // Before any set, the file system carries no policy at all.
    assert_policy_not_found(
        "the read before any set",
        &efs.describe_file_system_policy(&file_system_id).await,
    );

    for (case, document) in accepted_documents(&file_system_arn) {
        let submitted = document.to_string();
        let put = efs
            .put_file_system_policy(&file_system_id, json!({ "Policy": submitted }))
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutFileSystemPolicy expected 200, got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.string("FileSystemId"),
            file_system_id,
            "case {case:?}: the acceptance names the file system it applies to, got {:?}",
            put.get("FileSystemId")
        );
        // The reported document IS the submitted one: same policy, whatever the layout.
        let reported = put.string("Policy");
        let parsed: Value = serde_json::from_str(&reported)
            .unwrap_or_else(|error| panic!("case {case:?}: reported Policy is JSON: {error}"));
        assert_eq!(
            parsed, document,
            "case {case:?}: the reported policy parses equal to the submitted one, got \
             {reported:?}"
        );

        // ROUND TRIP: an independent read observes the stored document.
        let described = efs.describe_file_system_policy(&file_system_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeFileSystemPolicy expected 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.string("Policy"),
            reported,
            "case {case:?}: the read observes the stored document, got {:?}",
            described.get("Policy")
        );
        assert_eq!(
            described.string("FileSystemId"),
            file_system_id,
            "case {case:?}: the read names the file system, got {:?}",
            described.get("FileSystemId")
        );

        // The rendering is a fixed point: a client that puts back what it read sees no
        // drift, so a store/read loop terminates.
        let again = efs
            .put_file_system_policy(&file_system_id, json!({ "Policy": reported.clone() }))
            .await;
        assert_eq!(
            again.string("Policy"),
            reported,
            "case {case:?}: re-storing the reported document changes nothing, got {:?}",
            again.get("Policy")
        );
    }
}

#[tokio::test]
async fn deleting_the_policy_returns_the_file_system_to_the_default_and_is_idempotent() {
    // (case, the policy stored before the delete, or `None` for a file system that never
    // carried one). "The default FileSystemPolicy goes into effect once the existing
    // policy is deleted", which is the state a file system with no explicit policy is
    // already in — so the delete lands in the same state from either side.
    let efs = Emulator::new();
    let created = efs
        .create_file_system(json!({ "CreationToken": "policy-delete" }))
        .await;
    let file_system_id = created.string("FileSystemId");
    let file_system_arn = created.string("FileSystemArn");
    settle().await;

    let mut stored: Vec<(&str, Option<Value>)> = vec![("no policy was ever stored", None)];
    stored.extend(
        accepted_documents(&file_system_arn)
            .into_iter()
            .map(|(case, document)| (case, Some(document))),
    );

    for (case, document) in stored {
        if let Some(document) = &document {
            let put = efs
                .put_file_system_policy(
                    &file_system_id,
                    json!({ "Policy": document.to_string() }),
                )
                .await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: PutFileSystemPolicy expected 200, got {} {:?}",
                put.status, put.body
            );
        }

        // Deleting twice is deleting once: the operation models no not-found error for
        // the policy, so both calls are accepted and both leave the same state.
        for attempt in 1..=2 {
            let row = format!("{case} (delete {attempt})");
            let deleted = efs.delete_file_system_policy(&file_system_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {row:?}: DeleteFileSystemPolicy expected 200, got {} {:?}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body_len, 0,
                "case {row:?}: the modelled output is empty, got a {}-byte body",
                deleted.body_len
            );
            assert_policy_not_found(
                &row,
                &efs.describe_file_system_policy(&file_system_id).await,
            );
        }
    }

    // An id no file system in this account holds, and a file system that is not yet
    // available, are refused the same way the other policy entry points refuse them.
    let missing = "fs-00000000deadbeef";
    let refused = efs.delete_file_system_policy(missing).await;
    assert_eq!(
        refused.status, 404,
        "case the delete on an unknown file system: expected HTTP 404, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("Message"),
        state::file_system_not_found_message(missing),
        "case the delete on an unknown file system: unexpected Message, got {:?}",
        refused.get("Message")
    );

    let creating = efs
        .create_file_system(json!({ "CreationToken": "policy-delete-while-creating" }))
        .await;
    let creating_id = creating.string("FileSystemId");
    let refused = efs.delete_file_system_policy(&creating_id).await;
    assert_eq!(
        refused.status, 409,
        "case the delete on a creating file system: expected HTTP 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "case the delete on a creating file system: expected the modelled \
         IncorrectFileSystemLifeCycleState, got {:?}",
        refused.error_type
    );
    settle().await;
}

#[tokio::test]
async fn the_lockout_safety_check_refuses_a_deny_of_future_policy_updates() {
    let efs = Emulator::new();

    // One file system per row: a row's document must be decided by the request alone, so
    // it is never judged against a policy another row stored.
    for (index, (case, build)) in lockout_documents().into_iter().enumerate() {
        let created = efs
            .create_file_system(json!({ "CreationToken": format!("policy-lockout-{index}") }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");
        let file_system_arn = created.string("FileSystemArn");
        settle().await;

        // The row's document, built against THIS file system's own ARN.
        let document = build(&file_system_arn);
        let submitted = document.to_string();

        let refused = efs
            .put_file_system_policy(&file_system_id, json!({ "Policy": submitted.clone() }))
            .await;
        assert_invalid_policy(case, &refused, Some(LOCKOUT_MESSAGE));
        // Effect-free: the refusal left no policy behind.
        assert_policy_not_found(case, &efs.describe_file_system_policy(&file_system_id).await);

        // "Set BypassPolicyLockoutSafetyCheck to True only when you intend to prevent the
        // IAM principal that is making the request from making subsequent
        // PutFileSystemPolicy requests on this file system": the SAME document is
        // accepted when the request asks for the check to be bypassed.
        let bypassed = efs
            .put_file_system_policy(
                &file_system_id,
                json!({ "Policy": submitted, "BypassPolicyLockoutSafetyCheck": true }),
            )
            .await;
        assert_eq!(
            bypassed.status, 200,
            "case {case:?}: the bypassing request expected 200, got {} {:?}",
            bypassed.status, bypassed.body
        );
        let reported = bypassed.string("Policy");
        let parsed: Value = serde_json::from_str(&reported)
            .unwrap_or_else(|error| panic!("case {case:?}: reported Policy is JSON: {error}"));
        assert_eq!(
            parsed, document,
            "case {case:?}: the bypassing request stores the submitted document, got \
             {reported:?}"
        );
        let described = efs.describe_file_system_policy(&file_system_id).await;
        assert_eq!(
            described.string("Policy"),
            reported,
            "case {case:?}: the read observes the bypassed document, got {:?}",
            described.get("Policy")
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_a_resource_policy_is_refused_and_stores_nothing() {
    // (case, the `Policy` the request carries). Each row breaks one clause of the IAM
    // JSON policy grammar: the document is an object; a `Version` it declares is one the
    // language defines; `Statement` is required and holds at least one statement object;
    // and a RESOURCE-based statement carries an `Effect` of Allow or Deny, at least one
    // action and at least one principal.
    let malformed: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        ("a JSON scalar", "5".to_string()),
        ("a JSON array", "[]".to_string()),
        ("an object with no Statement", json!({ "Version": "2012-10-17" }).to_string()),
        (
            "an empty Statement list",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({ "Version": "2012-10-17", "Statement": ["Allow"] }).to_string(),
        ),
        (
            "a policy version that does not exist",
            json!({
                "Version": "2020-01-01",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "elasticfilesystem:ClientMount",
                }],
            })
            .to_string(),
        ),
        (
            "a statement with no Effect",
            policy(json!({
                "Principal": "*",
                "Action": "elasticfilesystem:ClientMount",
            }))
            .to_string(),
        ),
        (
            "a statement whose effect does not exist",
            policy(json!({
                "Effect": "Permit",
                "Principal": "*",
                "Action": "elasticfilesystem:ClientMount",
            }))
            .to_string(),
        ),
        (
            "a statement that names no action",
            policy(json!({ "Effect": "Allow", "Principal": "*" })).to_string(),
        ),
        (
            "a statement that names no principal",
            policy(json!({
                "Effect": "Allow",
                "Action": "elasticfilesystem:ClientMount",
            }))
            .to_string(),
        ),
    ];

    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("policy-grammar").await;

    for (case, document) in malformed {
        // The grammar is judged before the lockout safety check, so a bypass does not
        // make a document that is not a policy storable.
        for bypass in [false, true] {
            let row = format!("{case} (bypass {bypass})");
            let refused = efs
                .put_file_system_policy(
                    &file_system_id,
                    json!({ "Policy": document, "BypassPolicyLockoutSafetyCheck": bypass }),
                )
                .await;
            assert_invalid_policy(&row, &refused, None);
            assert_policy_not_found(
                &row,
                &efs.describe_file_system_policy(&file_system_id).await,
            );
        }
    }
}

#[tokio::test]
async fn the_file_system_policy_operations_refuse_what_they_cannot_serve() {
    let efs = Emulator::new();

    // An id no file system in this account holds: both entry points refuse with the one
    // request-derived sentence.
    let missing = "fs-00000000deadbeef";
    let open_policy = policy(json!({
        "Effect": "Allow",
        "Principal": "*",
        "Action": POLICY_MANAGEMENT_ACTION,
        "Resource": "*",
    }))
    .to_string();
    for (case, reply) in [
        (
            "the read of an unknown file system",
            efs.describe_file_system_policy(missing).await,
        ),
        (
            "a set on an unknown file system",
            efs.put_file_system_policy(missing, json!({ "Policy": open_policy.clone() }))
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("FileSystemNotFound"),
            "case {case:?}: expected the modelled FileSystemNotFound, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("Message"),
            state::file_system_not_found_message(missing),
            "case {case:?}: unexpected Message, got {:?}",
            reply.get("Message")
        );
    }

    // `IncorrectFileSystemLifeCycleState`: "Returned if the file system's lifecycle state
    // is not available" — a file system still `creating` takes no policy, and the refusal
    // stores nothing.
    let creating = efs
        .create_file_system(json!({ "CreationToken": "policy-while-creating" }))
        .await;
    let creating_id = creating.string("FileSystemId");
    let refused = efs
        .put_file_system_policy(&creating_id, json!({ "Policy": open_policy }))
        .await;
    assert_eq!(
        refused.status, 409,
        "case the set on a creating file system: expected HTTP 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "case the set on a creating file system: expected the modelled \
         IncorrectFileSystemLifeCycleState, got {:?}",
        refused.error_type
    );
    settle().await;
    assert_policy_not_found(
        "the read after a set refused for the lifecycle state",
        &efs.describe_file_system_policy(&creating_id).await,
    );
}
