// SPDX-License-Identifier: CC0-1.0

//! A sync job is created only when this service can act as the role the request
//! names, end to end over HTTP.
//!
//! `tests/all/sync_job.rs` covers what an accepted job IS — the record, the
//! clock-owed outcome, the listing, the conflict and the unsupported source.
//! This suite covers the rule that decides whether a create is accepted at all:
//! a sync job reads its sync source and writes the workspace AS the role
//! `syncRole` names, so a create naming a role of this account whose trust
//! policy withholds `sts:AssumeRole` from `iottwinmaker.amazonaws.com` is
//! refused SYNCHRONOUSLY — the modelled `ValidationException` (HTTP 400), not an
//! accepted job left to fail later — and persists nothing, so the `GetSyncJob`
//! behind it is a `ResourceNotFoundException`.
//!
//! The role is resolved against the account's IAM records, the very records
//! `CreateRole` wrote in the same engine, so the rows run over every form of the
//! question: the trust forms IAM admits (the action and the principal as strings
//! or lists, the wildcards, the casing), the forms that withhold the grant
//! (another principal, an explicit deny, another action, a statement list
//! granting nothing) and the case that is no evidence either way — an ARN the
//! IAM records hold no role for, which is accepted.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::SYNC_JOB_STATE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    sync_job_not_found_message, SERVICE_PRINCIPAL, SYNC_SOURCE_SITEWISE,
};

use crate::harness::Emulator;

/// A trust policy with one statement, written as a client would send it.
fn policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// The trust documents the rule generalises over, and whether a role carrying
/// one may be used: `(case, document, admitted)`.
///
/// `None` for the document is the row whose ARN names no role of this account at
/// all — not evidence of a missing trust, so the create goes ahead.
fn rows() -> Vec<(&'static str, Option<String>, bool)> {
    let principal = format!("\"{SERVICE_PRINCIPAL}\"");
    vec![
        (
            "the action and the principal as strings",
            Some(policy("Allow", "\"sts:AssumeRole\"", &principal)),
            true,
        ),
        (
            "both as lists",
            Some(policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                &format!("[\"ec2.amazonaws.com\",\"{SERVICE_PRINCIPAL}\"]"),
            )),
            true,
        ),
        (
            "a service wildcard in the action",
            Some(policy("Allow", "\"sts:*\"", &principal)),
            true,
        ),
        (
            "every action",
            Some(policy("Allow", "\"*\"", &principal)),
            true,
        ),
        (
            "the principal in another casing",
            Some(policy(
                "Allow",
                "\"STS:ASSUMEROLE\"",
                "\"IoTTwinMaker.amazonaws.com\"",
            )),
            true,
        ),
        (
            "another service's principal only",
            Some(policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\"")),
            false,
        ),
        (
            "an explicit deny",
            Some(policy("Deny", "\"sts:AssumeRole\"", &principal)),
            false,
        ),
        (
            "an action that is not assuming the role",
            Some(policy("Allow", "\"s3:GetObject\"", &principal)),
            false,
        ),
        (
            "a statement list granting nothing",
            Some("{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()),
            false,
        ),
        // There is no row for a trust document that is not a policy at all: IAM
        // itself refuses `CreateRole` for one ("AssumeRolePolicyDocument must be
        // a JSON object carrying a Statement element"), so no role of this
        // account can carry such a document.
        ("a role this account does not hold", None, true),
    ]
}

/// RULE: a create whose `syncRole` names a role of this account that cannot be
/// assumed by this service is refused 400 `ValidationException` and stores
/// nothing; any other role is accepted and the job reads back in a modelled
/// `SyncJobState`.
#[tokio::test]
async fn a_sync_role_this_service_cannot_assume_is_refused_and_persists_nothing() {
    let emulator = Emulator::with_iam();

    for (index, (case, document, admitted)) in rows().into_iter().enumerate() {
        // One workspace per row: a sync source identifies at most one job inside
        // a workspace, and the recorded refusal is about the create, so each row
        // needs a workspace of its own to create into.
        let workspace_id = format!("syncTrustWs{index}");
        assert_eq!(
            emulator.create_workspace(&workspace_id).await.status,
            200,
            "case {case:?}: the workspace the row creates into must exist"
        );
        let role_arn = match &document {
            Some(document) => {
                emulator
                    .create_iam_role(&format!("twinmaker-sync-{index}"), document)
                    .await
            }
            // The modelled `RoleArn` form, naming a role the IAM store holds no
            // record of.
            None => format!("arn:aws:iam::000000000000:role/unknown-to-iam-{index}"),
        };

        let created = emulator
            .create_sync_job(
                &workspace_id,
                SYNC_SOURCE_SITEWISE,
                json!({"syncRole": role_arn}),
            )
            .await;
        let read = emulator
            .get_sync_job(Some(&workspace_id), SYNC_SOURCE_SITEWISE)
            .await;

        if admitted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be served, got {created:?}"
            );
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created job must read back, got {read:?}"
            );
            let state = read.member("status")["state"].as_str().unwrap_or_default();
            assert!(
                SYNC_JOB_STATE_VALUES.contains(&state),
                "case {case:?}: expected a modelled SyncJobState, got {state:?} in {read:?}"
            );
            assert_eq!(
                read.member("syncRole").as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the job must carry the role the create named, got {read:?}"
            );
        } else {
            assert_eq!(
                (created.status, created.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected a 400 ValidationException, got {created:?}"
            );
            let message = created.member("message").as_str().unwrap_or_default();
            for expected in ["syncRole", role_arn.as_str(), SERVICE_PRINCIPAL] {
                assert!(
                    message.contains(expected),
                    "case {case:?}: the refusal must name {expected:?}, got {message:?}"
                );
            }
            // Nothing was persisted: the read behind the refused create is the
            // service's own not-found refusal for a sync job, and the workspace
            // reports no linked service either.
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a refused create must persist no job, got {read:?}"
            );
            assert_eq!(
                read.member("message").as_str(),
                Some(sync_job_not_found_message(&workspace_id, SYNC_SOURCE_SITEWISE).as_str()),
                "case {case:?}: expected the sync job not-found construction, got {read:?}"
            );
            let workspace = emulator.get_workspace(&workspace_id).await;
            assert!(
                workspace.member("linkedServices").is_null(),
                "case {case:?}: a refused create must link no service, got {workspace:?}"
            );
        }
    }
}

/// RULE (the same one, from the other side): the role is read from the IAM
/// records as they stand, so the SAME ARN is refused or accepted according to
/// the trust policy the account's role carries — not according to anything the
/// request spells.
#[tokio::test]
async fn the_verdict_follows_the_stored_trust_policy_not_the_arn() {
    let emulator = Emulator::with_iam();
    let refused_role = emulator
        .create_iam_role(
            "twinmaker-ec2-only",
            &policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\""),
        )
        .await;
    let accepted_role = emulator
        .create_iam_role(
            "twinmaker-sync",
            &policy("Allow", "\"sts:AssumeRole\"", &format!("\"{SERVICE_PRINCIPAL}\"")),
        )
        .await;
    assert_eq!(
        emulator.create_workspace("syncTrustWs").await.status,
        200,
        "the workspace both creates name must exist"
    );

    // The refused create leaves the source free, so the second create — which
    // differs only in the role it names — is not a conflict but the accepted
    // case.
    let refused = emulator
        .create_sync_job(
            "syncTrustWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": refused_role, "tags": {"team": "plant"}}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"unassumable role\": expected a 400 ValidationException, got {refused:?}"
    );
    let accepted = emulator
        .create_sync_job(
            "syncTrustWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": accepted_role, "tags": {"team": "plant"}}),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"assumable role\": expected the create to be served, got {accepted:?}"
    );
    let read = emulator
        .get_sync_job(Some("syncTrustWs"), SYNC_SOURCE_SITEWISE)
        .await;
    assert_eq!(
        read.member("syncRole").as_str(),
        Some(accepted_role.as_str()),
        "case \"assumable role\": the stored job must carry the accepted role, got {read:?}"
    );
    assert_eq!(
        read.member("arn").as_str(),
        accepted.member("arn").as_str(),
        "case \"assumable role\": the read must report the ARN the create minted, got {read:?}"
    );
}
