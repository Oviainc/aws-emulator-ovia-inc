// SPDX-License-Identifier: CC0-1.0

//! `PutBackupVaultAccessPolicy` / `GetBackupVaultAccessPolicy`: a vault's
//! resource policy is a STORED member of the vault, and the document a caller
//! submits has to be a policy before it can be stored.
//!
//! The rules under test:
//!
//! * `Policy` carries a document in the IAM JSON policy language, so a body
//!   outside that language is refused 400 `InvalidParameterValueException` and
//!   NOTHING is attached — the policy read keeps answering exactly as it did
//!   before the refused call;
//! * a document inside it is accepted with 200 and round-trips through the
//!   policy read, which reports it beside the vault it is attached to;
//! * a vault carries exactly one policy, so a later accepted document replaces
//!   the one before it, and a vault carrying none has none to report.
//!
//! The malformed rows below are one per clause of the grammar the tree reads a
//! resource policy by (`service_common::iam_policy`'s `PolicyDefect`), and the
//! well-formed rows span the forms that language admits: one statement or a
//! list of them, each element written as a string or as a list, both published
//! policy versions and no version at all, and the complement elements
//! (`NotAction`, `NotPrincipal`).
//!
//! Every case drives real HTTP through the router; the harness is the shared
//! one in `vault_and_backup_job.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{call, create_vault, error_code, router, string_at, ACCOUNT, REGION};

/// The sentence AWS states for "the document offered as this vault's resource
/// policy is not a policy" — fixed text, naming neither the vault nor the
/// document.
const POLICY_INVALID: &str =
    "Provided Policy is not valid. Please input a valid policy and retry the request.";

/// The FIXED text the policy read refuses a vault carrying no policy with: the
/// vault's own ARN is the one variable part.
fn no_policy_message(vault: &str) -> String {
    format!("Vault arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{vault} has no associated vault policy")
}

/// `PUT /backup-vaults/{BackupVaultName}/access-policy`, as the model binds the
/// write. The body is built here rather than by the caller so a case can send a
/// `Policy` that is not even JSON.
async fn put_policy(router: &MultiServiceRouter, vault: &str, policy: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{vault}/access-policy"),
        Some(&json!({"Policy": policy}).to_string()),
    )
    .await
}

/// `GET /backup-vaults/{BackupVaultName}/access-policy`, as the model binds the
/// read.
async fn get_policy(router: &MultiServiceRouter, vault: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/backup-vaults/{vault}/access-policy"),
        None,
    )
    .await
}

/// A well-formed resource policy whose statement names `principal` and allows
/// `action` on this vault's recovery points.
fn policy_allowing(principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "allow",
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": action,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The ARN of a principal of this account.
fn principal_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Create the vault every case in this suite attaches a policy to.
async fn vault_named(router: &MultiServiceRouter, name: &str) {
    let (status, created) = create_vault(router, name, "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault {name} should succeed, got {status} {created:?}"
    );
}

/// A document outside the IAM JSON policy language is refused as the value of
/// the member that carried it, and the vault is left carrying no policy.
///
/// One row per clause of the grammar, so the rule is asserted over the class
/// rather than over one hand-picked body.
#[tokio::test]
async fn a_body_that_is_not_a_policy_document_is_refused_and_attaches_nothing() {
    let cases: Vec<(&str, String)> = vec![
        ("not JSON at all", "<not a valid IAM policy document>".to_string()),
        ("an empty body", String::new()),
        ("a JSON scalar", "5".to_string()),
        ("a JSON string", "\"a policy\"".to_string()),
        ("a JSON array", "[]".to_string()),
        ("an object with no statement", json!({"Version": "2012-10-17"}).to_string()),
        (
            "a policy version that does not exist",
            json!({
                "Version": "2525-01-01",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": principal_arn("reader")},
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
        (
            "an empty statement list",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({"Version": "2012-10-17", "Statement": ["allow everything"]}).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({
                "Statement": [{
                    "Principal": {"AWS": principal_arn("reader")},
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Effect is neither Allow nor Deny",
            json!({
                "Statement": [{
                    "Effect": "Maybe",
                    "Principal": {"AWS": principal_arn("reader")},
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
        (
            "a statement naming no action",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": principal_arn("reader")},
                }],
            })
            .to_string(),
        ),
        (
            "a statement naming no principal",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
    ];

    let router = router();
    vault_named(&router, "policy-vault").await;

    for (case, policy) in cases {
        let (status, refused) = put_policy(&router, "policy-vault", &policy).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "InvalidParameterValueException", POLICY_INVALID),
            "case {case:?} ({policy:?}): expected the parameter refusal, got {status} {refused:?}"
        );
        // Nothing was attached, so the read answers as it did before the call.
        let (status, read) = get_policy(&router, "policy-vault").await;
        assert_eq!(
            (
                status,
                error_code(&read).as_str(),
                string_at(&read, "Message")
            ),
            (
                400,
                "ResourceNotFoundException",
                no_policy_message("policy-vault")
            ),
            "case {case:?}: a refused put must attach nothing, got {status} {read:?}"
        );
    }
}

/// A document inside the language is accepted and round-trips: the read reports
/// the same document, attached to the vault that was named.
///
/// The rows differ along the forms the language admits, not along one policy
/// written several ways.
#[tokio::test]
async fn an_accepted_policy_round_trips_through_the_policy_read() {
    let cases: Vec<(&str, String)> = vec![
        (
            "one statement in a list",
            policy_allowing(&principal_arn("reader"), "backup:DescribeBackupVault"),
        ),
        (
            "one statement written as an object",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": principal_arn("single")},
                    "Action": "backup:DescribeRecoveryPoint",
                },
            })
            .to_string(),
        ),
        (
            "two statements, one of each effect",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": principal_arn("reader")},
                        "Action": ["backup:DescribeBackupVault", "backup:ListRecoveryPointsByBackupVault"],
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "backup:DeleteRecoveryPoint",
                    },
                ],
            })
            .to_string(),
        ),
        (
            "no version at all",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": principal_arn("versionless")},
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
        (
            "the legacy policy version",
            json!({
                "Version": "2008-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": principal_arn("legacy")},
                    "Action": "backup:DescribeBackupVault",
                }],
            })
            .to_string(),
        ),
        (
            "the complement elements",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": principal_arn("owner")},
                    "NotAction": "backup:DescribeBackupVault",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
    ];

    for (case, policy) in cases {
        let router = router();
        vault_named(&router, "policy-vault").await;

        let (status, accepted) = put_policy(&router, "policy-vault", &policy).await;
        assert_eq!(
            (status, error_code(&accepted).as_str()),
            (200, "<none>"),
            "case {case:?}: expected PutBackupVaultAccessPolicy 200, got {status} {accepted:?}"
        );

        let (status, read) = get_policy(&router, "policy-vault").await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected GetBackupVaultAccessPolicy 200, got {status} {read:?}"
        );
        assert_eq!(
            string_at(&read, "BackupVaultName"),
            "policy-vault",
            "case {case:?}: the read must name the vault it answered for, got {read:?}"
        );
        assert_eq!(
            string_at(&read, "BackupVaultArn"),
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:policy-vault"),
            "case {case:?}: the read must report the vault's own ARN, got {read:?}"
        );
        // The stored document is reported in the one layout an attached policy
        // is reported in, so the DOCUMENT is compared rather than its bytes.
        let reported: Value = serde_json::from_str(&string_at(&read, "Policy"))
            .unwrap_or_else(|error| panic!("case {case:?}: the reported policy must be JSON: {error}"));
        let submitted: Value =
            serde_json::from_str(&policy).expect("a well-formed policy literal in the table");
        assert_eq!(
            reported, submitted,
            "case {case:?}: the read must report the document that was attached"
        );
    }
}

/// A vault carries exactly one policy: a second accepted document replaces the
/// first, and a vault that was never given one has none to report — which is
/// the same refusal a vault whose policy was replaced by none answers.
#[tokio::test]
async fn a_vault_carries_exactly_one_policy_and_a_later_document_replaces_it() {
    let router = router();
    vault_named(&router, "policy-vault").await;
    vault_named(&router, "bare-vault").await;

    let (status, read) = get_policy(&router, "bare-vault").await;
    assert_eq!(
        (
            status,
            error_code(&read).as_str(),
            string_at(&read, "Message")
        ),
        (
            400,
            "ResourceNotFoundException",
            no_policy_message("bare-vault")
        ),
        "a vault that was never given a policy has none to report, got {status} {read:?}"
    );

    let first = policy_allowing(&principal_arn("first"), "backup:DescribeBackupVault");
    let second = policy_allowing(&principal_arn("second"), "backup:DeleteRecoveryPoint");
    for (case, policy) in [("the first document", &first), ("the second document", &second)] {
        let (status, accepted) = put_policy(&router, "policy-vault", policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {accepted:?}"
        );
        let (status, read) = get_policy(&router, "policy-vault").await;
        assert_eq!(status, 200, "case {case:?}: expected the policy read to answer 200");
        let reported: Value = serde_json::from_str(&string_at(&read, "Policy"))
            .expect("the reported policy is JSON");
        let submitted: Value = serde_json::from_str(policy).expect("a well-formed policy literal");
        assert_eq!(
            reported, submitted,
            "case {case:?}: the read must report the document in force"
        );
    }

    // The vault the suite never wrote to is still carrying nothing: the write
    // is per vault.
    let (status, read) = get_policy(&router, "bare-vault").await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (400, "ResourceNotFoundException"),
        "another vault's policy must not reach this one, got {status} {read:?}"
    );
}

/// Both halves decide access before existence: a vault this account does not
/// hold is refused for lack of privileges rather than reported absent.
#[tokio::test]
async fn a_vault_the_caller_cannot_see_is_refused_by_both_halves() {
    let router = router();
    let policy = policy_allowing(&principal_arn("reader"), "backup:DescribeBackupVault");

    let (status, refused) = put_policy(&router, "never-created", &policy).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "the write must refuse an invisible vault for lack of privileges, got {status} {refused:?}"
    );
    let (status, refused) = get_policy(&router, "never-created").await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "the read must refuse an invisible vault for lack of privileges, got {status} {refused:?}"
    );
}
