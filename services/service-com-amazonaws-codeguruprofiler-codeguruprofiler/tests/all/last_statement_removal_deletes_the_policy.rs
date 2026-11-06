// SPDX-License-Identifier: CC0-1.0

//! What a `RemovePermission` that takes a policy's LAST statement leaves behind, end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rule every row asserts: a profiling group holds a resource-based policy exactly
//! while that policy holds at least one statement, so the removal that revokes the last
//! statement DELETES the policy and returns the group to the policy-less state it was
//! created in — that removal's own 200 carries neither `policy` nor `revisionId`, and
//! `GetPolicy` then answers 200 carrying neither either, a policy-scoped removal finds
//! no policy to remove from (404 `ResourceNotFoundException`, naming the group and the
//! account the lookup was scoped to), and the next grant is a FIRST write again, which
//! names no revision. A removal that leaves a statement behind changes nothing about the
//! policy's existence: it is still stored, at the revision the removal minted and
//! reported.
//!
//! Rows differ along what the rule generalises over: how many statements the policy
//! holds when the removal arrives (1, 2 and 3 — so the removal leaves 0, 1 and 2
//! behind), how many principals the removed statement granted to (1, 2 and 50, the
//! modelled `Principals` bounds being 1..50), and the profiling group the policy sits on
//! (the shared group table spans the modelled `ProfilingGroupName` length bounds 1..255).
//! Nothing here is keyed to the number of statements of any one recorded case: the
//! 0-left rows and the statements-left rows run through the same walk, which branches
//! only on whether a statement survived.
//!
//! A policy holding more than one statement cannot be written over HTTP (the modelled
//! `ActionGroup` enum declares one value), so those rows are seeded into the store
//! through the same state writer [`crate::resource_policy::attach_policy`] uses. Every
//! removal, read and re-grant goes over HTTP.

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::ACTION_GROUP_AGENT_PERMISSIONS;

use crate::permission_writes::{
    get_policy, is_revision_id, put_permission, remove_permission, string_member,
};
use crate::profiling_group_lifecycle::{ACCOUNT, Answer, GroupCase, Harness, group_cases};
use crate::resource_policy::attach_policy;

/// The action group every row removes: the one value the modelled `ActionGroup` enum
/// declares (`smithy_query(query="shape",shape="ActionGroup")`), which is also the only
/// one a policy write over HTTP admits.
const REMOVED_ACTION_GROUP: &str = ACTION_GROUP_AGENT_PERMISSIONS;

/// Action groups a seeded policy carries BESIDE the removed one, so a row can hold more
/// than one statement. These are not modelled enum values — they cannot be written over
/// HTTP — and what they contribute is a statement the removal must leave behind, which
/// keeps the policy in existence.
const COMPANION_ACTION_GROUPS: &[&str] = &["internalAgentPermissions", "consolePermissions"];

/// Well-formed `RevisionId`s this store never minted
/// (`smithy_query(query="shape",shape="RevisionId")`:
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`), for the calls that
/// name a revision the store does not hold.
const FOREIGN_REVISIONS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "0e5b1ecf-1922-8afe-2c40-ef410afc120a",
];

/// Principal ARNs a grant names: the forms the `principals` doc names ("A list ARNs for
/// the roles and users you want to grant access to the profiling group"), then distinct
/// role ARNs past that catalogue so a row can reach the modelled maximum. The account id
/// is the emulator's own.
fn principal(index: usize) -> String {
    let forms = [
        format!("arn:aws:iam::{ACCOUNT}:role/cgp-last-agent"),
        format!("arn:aws:iam::{ACCOUNT}:user/cgp-last-analyst"),
        format!("arn:aws:iam::{ACCOUNT}:root"),
        format!("arn:aws:iam::{ACCOUNT}:role/service-role/cgp-last-lambda"),
        format!("arn:aws:sts::{ACCOUNT}:assumed-role/cgp-last-agent/session"),
    ];
    forms
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("arn:aws:iam::{ACCOUNT}:role/cgp-last-bulk-{index}"))
}

fn principals(count: usize) -> Vec<String> {
    (0..count).map(principal).collect()
}

/// One row: the group the policy sits on, the statements the policy holds when the
/// removal arrives (the removed action group first), and the revision it is seeded at.
#[derive(Debug)]
struct Case {
    group: GroupCase,
    /// `actionGroup` -> the principals it grants to. The first entry is the statement
    /// the row removes; the rest are the statements that must survive it.
    held: Vec<(String, Vec<String>)>,
    /// The revision the seeded policy carries, in the modelled `RevisionId` form.
    revision_id: String,
}

impl Case {
    /// How many statements the removal leaves behind — and so whether the policy itself
    /// survives it.
    fn statements_left(&self) -> usize {
        self.held.len() - 1
    }
}

/// The table: every row of the shared group table, crossed with a statement count (1, 2
/// and 3, so the removal leaves 0, 1 and 2 behind) and a principal count spanning the
/// modelled `Principals` bounds (1 and 50) and a count between.
fn cases() -> Vec<Case> {
    // (statements the policy holds, principals the removed statement grants to, the
    // revision the policy is seeded at).
    let shapes: [(usize, usize, &str); 5] = [
        (1, 1, "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d"),
        (2, 2, "eeeeeeee-1111-2222-3333-444455556666"),
        (3, 50, "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
        (1, 50, "abcdef01-2345-6789-abcd-ef0123456789"),
        (1, 2, "beefbeef-beef-beef-beef-beefbeefbeef"),
    ];
    let groups = group_cases();
    assert_eq!(
        groups.len(),
        shapes.len(),
        "every shared group row must carry a policy shape, got {} group(s) and {} \
         shape(s)",
        groups.len(),
        shapes.len()
    );

    groups
        .into_iter()
        .zip(shapes)
        .map(|(group, (statements, principal_count, revision_id))| {
            let mut held = vec![(
                REMOVED_ACTION_GROUP.to_string(),
                principals(principal_count),
            )];
            for index in 0..statements - 1 {
                let action_group = COMPANION_ACTION_GROUPS
                    .get(index)
                    .unwrap_or_else(|| panic!("no companion action group {index}"));
                held.push((action_group.to_string(), principals(index + 1)));
            }
            Case {
                group,
                held,
                revision_id: revision_id.to_string(),
            }
        })
        .collect()
}

/// Create every row's group and seed its policy, so every row is stored at once: a row
/// that answered from another row's state fails here rather than being hidden by a fresh
/// store.
async fn seeded(harness: &Harness) -> Vec<Case> {
    let cases = cases();
    for (index, case) in cases.iter().enumerate() {
        let created = harness
            .create(&case.group, &format!("last-stmt-tok-{index}"))
            .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the group must be created first, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        attach_policy(harness, &case.group.name, &case.held, &case.revision_id);
    }
    cases
}

/// The number of statements `answer`'s `policy` member carries.
fn statement_count(label: &str, answer: &Answer) -> usize {
    let document = string_member(answer, "policy");
    let parsed: serde_json::Value = serde_json::from_str(&document)
        .unwrap_or_else(|error| panic!("case {label}: policy is not JSON ({error}): {document}"));
    parsed
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("case {label}: no Statement array in {document}"))
        .len()
}

/// The 404 a policy-scoped call answers a group that has NO policy with: the group as
/// the request named it and the account the lookup was scoped to.
fn no_policy_message(group: &str) -> String {
    format!("No policy attached to profiling group name: {group} in accountId: {ACCOUNT}; ")
}

/// Assert a policy-scoped removal was refused because the group holds no policy at all.
fn assert_no_policy_refusal(label: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        404,
        "case {label}: a removal from a policy-less group must answer 404, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {label}: expected ResourceNotFoundException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn a_removal_that_takes_the_last_statement_deletes_the_group_s_policy() {
    let harness = Harness::new();

    for case in seeded(&harness).await {
        let name = case.group.name.as_str();
        let label = format!("{name} holding {} statement(s)", case.held.len());

        let removed = remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id)
            .await;
        assert_eq!(
            removed.status,
            200,
            "case {label}: a removal naming the stored revision must be applied, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );

        let read = get_policy(&harness, name).await;
        assert_eq!(
            read.status,
            200,
            "case {label}: the group exists, so GetPolicy must answer 200, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );

        if case.statements_left() > 0 {
            // A statement survived, so the policy did: the removal reports the document
            // it left and the revision it minted, the policy is stored at that revision,
            // and the reader reports exactly what the removal reported.
            let revision = string_member(&removed, "revisionId");
            assert!(
                is_revision_id(&revision) && revision != case.revision_id,
                "case {label}: an applied removal must mint a fresh revision, got \
                 {revision:?} after {:?}",
                case.revision_id
            );
            assert_eq!(
                statement_count(&label, &removed),
                case.statements_left(),
                "case {label}: expected {} statement(s) in the document the removal left, \
                 got {}",
                case.statements_left(),
                string_member(&removed, "policy")
            );
            assert_eq!(
                harness.stored_policy_revision(name),
                Some(revision.clone()),
                "case {label}: a policy that still holds a statement must be stored at \
                 the revision the removal minted, got {:?}",
                harness.stored_policy_revision(name)
            );
            assert_eq!(
                read.json(),
                removed.json(),
                "case {label}: GetPolicy must report the policy the removal left, got {}",
                String::from_utf8_lossy(&read.body)
            );
            // The policy exists but no longer grants the removed action group, so a
            // second removal at the CURRENT revision is refused about the STATEMENT,
            // never with the no-policy sentence.
            let again = remove_permission(&harness, name, REMOVED_ACTION_GROUP, &revision).await;
            assert_eq!(
                again.status,
                404,
                "case {label}: removing a statement the policy does not hold must answer \
                 404, got {} {}",
                again.status,
                String::from_utf8_lossy(&again.body)
            );
            assert_ne!(
                again.error_message(),
                no_policy_message(name),
                "case {label}: the policy still exists, so the refusal must not say the \
                 group has none, got {:?}",
                again.error_message()
            );
            continue;
        }

        // The removal took the last statement, so the policy is GONE: nothing is stored,
        // and NEITHER the removal nor the read carries a member — equality against the
        // empty object says both that nothing is reported and that nothing else crept in.
        assert_eq!(
            harness.stored_policy_revision(name),
            None,
            "case {label}: the removal took the last statement, so no policy may remain \
             stored, got {:?}",
            harness.stored_policy_revision(name)
        );
        assert_eq!(
            removed.json(),
            serde_json::json!({}),
            "case {label}: a removal that deleted the policy must report neither policy \
             nor revisionId, got {}",
            String::from_utf8_lossy(&removed.body)
        );
        assert_eq!(
            read.json(),
            serde_json::json!({}),
            "case {label}: a group whose policy was deleted must report neither policy \
             nor revisionId, got {}",
            String::from_utf8_lossy(&read.body)
        );

        // There is no policy, so every revision — the one the removal consumed and any
        // other well-formed one — is answered the same way: a removal has no policy to
        // remove from.
        let mut revisions: Vec<String> = vec![case.revision_id.clone()];
        revisions.extend(FOREIGN_REVISIONS.iter().map(|value| value.to_string()));
        for sent in &revisions {
            let answer = remove_permission(&harness, name, REMOVED_ACTION_GROUP, sent).await;
            let case_label = format!("{label}, removal at {sent}");
            assert_no_policy_refusal(&case_label, &answer);
            assert_eq!(
                answer.error_message(),
                no_policy_message(name),
                "case {case_label}: expected {:?}, got {:?}",
                no_policy_message(name),
                answer.error_message()
            );
        }

        // ...and a grant that names a revision names one of a policy that does not
        // exist, while the grant that names NONE is the FIRST write it now is.
        for sent in &revisions {
            let answer =
                put_permission(&harness, name, REMOVED_ACTION_GROUP, &principals(1), Some(sent))
                    .await;
            assert_eq!(
                answer.status,
                409,
                "case {label}, grant at {sent}: naming a revision when no policy exists \
                 must conflict, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                harness.stored_policy_revision(name),
                None,
                "case {label}, grant at {sent}: a refused grant must create no policy"
            );
        }
    }
}

#[tokio::test]
async fn a_group_whose_policy_was_deleted_takes_a_first_write_again() {
    let harness = Harness::new();

    for case in seeded(&harness)
        .await
        .into_iter()
        .filter(|case| case.statements_left() == 0)
    {
        let name = case.group.name.as_str();
        let mut current = case.revision_id.clone();

        // Two grant/remove cycles: the state after the first cycle's removal must be the
        // state the group started in, so the second cycle behaves identically. Anything
        // the removal left behind would show up as a different answer in round 1.
        for round in 0..2 {
            let removed = remove_permission(&harness, name, REMOVED_ACTION_GROUP, &current).await;
            assert_eq!(
                removed.status,
                200,
                "case {name} round {round}: the removal must be applied, got {} {}",
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
            let consumed = current.clone();
            assert_eq!(
                removed.json(),
                serde_json::json!({}),
                "case {name} round {round}: the removal deleted the policy, so it must \
                 report neither policy nor revisionId, got {}",
                String::from_utf8_lossy(&removed.body)
            );
            assert_eq!(
                get_policy(&harness, name).await.json(),
                serde_json::json!({}),
                "case {name} round {round}: the policy must be gone"
            );

            // The first write on a group that has no policy names no revision.
            let regranted =
                put_permission(&harness, name, REMOVED_ACTION_GROUP, &principals(2), None).await;
            assert_eq!(
                regranted.status,
                200,
                "case {name} round {round}: a group with no policy must take a write that \
                 names no revision, got {} {}",
                regranted.status,
                String::from_utf8_lossy(&regranted.body)
            );
            let granted_revision = string_member(&regranted, "revisionId");
            assert!(
                is_revision_id(&granted_revision) && granted_revision != consumed,
                "case {name} round {round}: the re-grant must mint a fresh revision, got \
                 {granted_revision:?} after {consumed:?}"
            );
            // Round trip: the re-grant reads back through the reader, document and
            // revision alike.
            let read = get_policy(&harness, name).await;
            assert_eq!(
                read.json(),
                regranted.json(),
                "case {name} round {round}: GetPolicy must report what the re-grant \
                 stored, got {}",
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                statement_count(&format!("{name} round {round}"), &read),
                1,
                "case {name} round {round}: the re-granted policy must hold one \
                 statement, got {}",
                string_member(&read, "policy")
            );
            assert_eq!(
                harness.stored_policy_revision(name),
                Some(granted_revision.clone()),
                "case {name} round {round}: the re-granted policy must be stored, got {:?}",
                harness.stored_policy_revision(name)
            );
            current = granted_revision;
        }
    }
}
