// SPDX-License-Identifier: CC0-1.0

//! WHICH members a resource-policy answer serializes, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every row asserts: a profiling group holds a resource-based policy exactly
//! while that policy holds at least one statement, and an operation that reports a
//! policy (`PutPermission`, `RemovePermission`, `GetPolicy`) serializes `policy` and
//! `revisionId` only when the group holds one once the call is done — so the removal
//! that takes the LAST statement deletes the policy and answers 200 carrying NEITHER
//! member, exactly as `GetPolicy` then does on the same group, while a removal that
//! leaves a statement behind reports the document it left and the revision it minted.
//! A policy that does not exist is at no revision, so a deleting removal mints none.
//!
//! Rows differ along what the rule generalises over: how many statements the policy
//! holds when the removal arrives (1, 2 and 3 — so the removal leaves 0, 1 and 2
//! behind), how many principals the removed statement granted to (1, 2, 3 and 50, the
//! modelled `Principals` bounds being 1..50), whether the policy was written over HTTP
//! or seeded, and the profiling group it sits on (the shared group table spans the
//! modelled `ProfilingGroupName` length bounds 1..255). The rows that leave 0 and the
//! rows that leave statements run through the same walk, which branches only on whether
//! a statement survived.
//!
//! A policy holding more than one statement cannot be written over HTTP (the modelled
//! `ActionGroup` enum declares one value), so those rows are seeded into the store
//! through the same state writer [`crate::resource_policy::attach_policy`] uses. Every
//! removal, read and re-grant goes over HTTP.

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::ACTION_GROUP_AGENT_PERMISSIONS;

use crate::minted_ids_follow_applied_writes::mint_counts;
use crate::permission_writes::{
    get_policy, is_revision_id, put_permission, remove_permission, string_member,
};
use crate::profiling_group_lifecycle::{Answer, GroupCase, Harness, group_cases};
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

/// The two members a resource-policy answer can carry, in the model's member order
/// (`smithy_query(query="shape",shape="RemovePermissionResponse,GetPolicyResponse")`).
const POLICY_MEMBERS: [&str; 2] = ["policy", "revisionId"];

/// Principal ARNs a grant names: the forms the `principals` doc names ("A list ARNs for
/// the roles and users you want to grant access to the profiling group"), then distinct
/// role ARNs past that catalogue so a row can reach the modelled maximum. The account id
/// is the emulator's own.
fn principal(index: usize) -> String {
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    let forms = [
        format!("arn:aws:iam::{account}:role/cgp-absent-agent"),
        format!("arn:aws:iam::{account}:user/cgp-absent-analyst"),
        format!("arn:aws:iam::{account}:root"),
        format!("arn:aws:iam::{account}:role/service-role/cgp-absent-lambda"),
        format!("arn:aws:sts::{account}:assumed-role/cgp-absent-agent/session"),
    ];
    forms
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("arn:aws:iam::{account}:role/cgp-absent-bulk-{index}"))
}

fn principals(count: usize) -> Vec<String> {
    (0..count).map(principal).collect()
}

/// One row: the group the policy sits on, the statements it holds when the removal
/// arrives (the removed action group first), the revision it is at, and whether that
/// policy was written over HTTP rather than seeded.
#[derive(Debug)]
struct Case {
    group: GroupCase,
    /// `actionGroup` -> the principals it grants to. The first entry is the statement
    /// the row removes; the rest are the ones that must survive it.
    held: Vec<(String, Vec<String>)>,
    /// The revision the policy carries when the removal arrives, in the modelled
    /// `RevisionId` form.
    revision_id: String,
    /// Whether the policy under test was written through `PutPermission` over HTTP (a
    /// one-statement policy) rather than seeded into the store.
    written_over_http: bool,
}

impl Case {
    /// How many statements the removal leaves behind — and so whether the policy itself
    /// survives it.
    fn statements_left(&self) -> usize {
        self.held.len() - 1
    }

    /// The members the removal's answer must carry: none when the removal deletes the
    /// policy, both when a statement survives it.
    fn expected_members(&self) -> Vec<String> {
        if self.statements_left() == 0 {
            Vec::new()
        } else {
            POLICY_MEMBERS.iter().map(|m| (*m).to_string()).collect()
        }
    }
}

/// The members `answer`'s JSON body carries, in body order.
fn reported_members(label: &str, answer: &Answer) -> Vec<String> {
    answer
        .json()
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "case {label}: the answer must be a JSON object, got {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .keys()
        .cloned()
        .collect()
}

/// How many statements `answer`'s `policy` member carries.
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

/// The table: every row of the shared group table, crossed with a statement count (1, 2
/// and 3, so the removal leaves 0, 1 and 2 behind), a principal count spanning the
/// modelled `Principals` bounds (1 and 50) and counts between, and — on the
/// one-statement rows — whether the policy was written over HTTP or seeded.
fn cases() -> Vec<Case> {
    // (statements the policy holds, principals the removed statement grants to, the
    // revision a SEEDED policy is at, whether the policy is written over HTTP instead).
    let shapes: [(usize, usize, &str, bool); 5] = [
        (1, 1, "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d", false),
        (2, 2, "aaaabbbb-1111-2222-3333-444455556666", false),
        (3, 50, "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0", false),
        (1, 50, "cdef0123-4567-89ab-cdef-0123456789ab", true),
        (1, 3, "beefbeef-beef-beef-beef-beefbeefbeef", true),
    ];
    let groups = group_cases();
    assert_eq!(
        groups.len(),
        shapes.len(),
        "every shared group row must carry a policy shape, got {} group(s) and {} shape(s)",
        groups.len(),
        shapes.len()
    );

    groups
        .into_iter()
        .zip(shapes)
        .map(
            |(group, (statements, principal_count, revision_id, written_over_http))| {
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
                    written_over_http,
                }
            },
        )
        .collect()
}

/// Create every row's group and put its policy in place — over HTTP where the row says
/// so, seeded otherwise — so every row is stored at once: a row that answered from
/// another row's state fails here rather than being hidden by a fresh store.
async fn seeded(harness: &Harness) -> Vec<Case> {
    let mut cases = cases();
    for (index, case) in cases.iter_mut().enumerate() {
        let created = harness
            .create(&case.group, &format!("no-policy-tok-{index}"))
            .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the group must be created first, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        if case.written_over_http {
            let written = put_permission(
                harness,
                &case.group.name,
                REMOVED_ACTION_GROUP,
                &case.held[0].1,
                None,
            )
            .await;
            assert_eq!(
                written.status,
                200,
                "case {case:?}: the first grant must be applied, got {} {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            // A policy written over HTTP is at the revision that write minted, not at
            // any revision this table chose.
            case.revision_id = string_member(&written, "revisionId");
            continue;
        }
        attach_policy(harness, &case.group.name, &case.held, &case.revision_id);
    }
    cases
}

#[tokio::test]
async fn an_answer_carries_policy_and_revision_only_while_the_group_holds_a_policy() {
    let harness = Harness::new();

    for case in seeded(&harness).await {
        let name = case.group.name.as_str();
        let label = format!("{name} holding {} statement(s)", case.held.len());

        // A read before the removal reports the policy that is there: both members, so
        // the row's own "neither member" assertion below is about the REMOVAL, not about
        // a reader that never reports anything.
        let before = get_policy(&harness, name).await;
        assert_eq!(
            reported_members(&label, &before),
            POLICY_MEMBERS.to_vec(),
            "case {label}: a group that holds a policy must report both members, got {}",
            String::from_utf8_lossy(&before.body)
        );

        let minted_before = mint_counts(&harness);
        let removed =
            remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id).await;
        assert_eq!(
            removed.status,
            200,
            "case {label}: a removal naming the stored revision must be applied, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        assert_eq!(
            reported_members(&label, &removed),
            case.expected_members(),
            "case {label}: expected the members {:?}, got {}",
            case.expected_members(),
            String::from_utf8_lossy(&removed.body)
        );

        // Whatever the group holds now, the READ reports exactly what the removal
        // reported: the same policy, or the same nothing.
        let read = get_policy(&harness, name).await;
        assert_eq!(
            read.status,
            200,
            "case {label}: the group exists, so GetPolicy must answer 200, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json(),
            removed.json(),
            "case {label}: GetPolicy must report what the removal left, got {}",
            String::from_utf8_lossy(&read.body)
        );

        let minted_after = mint_counts(&harness);
        if case.statements_left() == 0 {
            // The policy is GONE: nothing is stored for it, nothing is reported of it,
            // and there is no policy to be at a revision, so none was minted.
            assert_eq!(
                read.json(),
                serde_json::json!({}),
                "case {label}: a removal that took the last statement must report \
                 neither member, got {}",
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                harness.stored_policy_revision(name),
                None,
                "case {label}: no policy may remain stored, got {:?}",
                harness.stored_policy_revision(name)
            );
            assert_eq!(
                minted_after, minted_before,
                "case {label}: a removal that deleted the policy must mint no revision; \
                 the store's (channelIds, revisions) counters moved from \
                 {minted_before:?} to {minted_after:?}"
            );

            // The group is policy-less exactly as it was created, so the next grant is a
            // FIRST write, which names no revision — and reads back through the reader.
            let regranted =
                put_permission(&harness, name, REMOVED_ACTION_GROUP, &principals(2), None).await;
            assert_eq!(
                regranted.status,
                200,
                "case {label}: a group with no policy must take a grant that names no \
                 revision, got {} {}",
                regranted.status,
                String::from_utf8_lossy(&regranted.body)
            );
            assert_eq!(
                get_policy(&harness, name).await.json(),
                regranted.json(),
                "case {label}: GetPolicy must report what the re-grant stored"
            );
            continue;
        }

        // A statement survived, so the policy did: it is stored at the revision the
        // removal minted and reported, and exactly one revision was minted for it.
        let revision = string_member(&removed, "revisionId");
        assert!(
            is_revision_id(&revision) && revision != case.revision_id,
            "case {label}: an applied removal must mint a fresh revision of the modelled \
             form, got {revision:?} after {:?}",
            case.revision_id
        );
        assert_eq!(
            minted_after,
            (minted_before.0, minted_before.1 + 1),
            "case {label}: a removal that left a policy behind must mint exactly one \
             revision, got {minted_after:?} from {minted_before:?}"
        );
        assert_eq!(
            harness.stored_policy_revision(name),
            Some(revision.clone()),
            "case {label}: the surviving policy must be stored at the revision the \
             removal reported, got {:?}",
            harness.stored_policy_revision(name)
        );
        assert_eq!(
            statement_count(&label, &removed),
            case.statements_left(),
            "case {label}: expected {} statement(s) in the document the removal left, \
             got {}",
            case.statements_left(),
            string_member(&removed, "policy")
        );
    }
}

#[tokio::test]
async fn a_grant_and_a_deleting_removal_return_the_group_to_where_it_started() {
    let harness = Harness::new();

    // Only the rows whose policy the removal empties: those are the ones the rule's
    // boundary sits on, and the cycle is driven entirely over HTTP.
    for case in seeded(&harness)
        .await
        .into_iter()
        .filter(|case| case.statements_left() == 0)
    {
        let name = case.group.name.as_str();
        let mut current = case.revision_id.clone();

        // Two grant/remove cycles: the state after the first cycle's removal must be the
        // state the group started in, so the second cycle answers identically — anything
        // the deleting removal left behind shows up as a different answer in round 1.
        for round in 0..2 {
            let removed = remove_permission(&harness, name, REMOVED_ACTION_GROUP, &current).await;
            assert_eq!(
                removed.status,
                200,
                "case {name} round {round}: the removal must be applied, got {} {}",
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
            assert_eq!(
                removed.json(),
                serde_json::json!({}),
                "case {name} round {round}: the removal deleted the policy, so it must \
                 report neither member, got {}",
                String::from_utf8_lossy(&removed.body)
            );
            // A read does not change what a later caller sees: both reads report the
            // same nothing.
            for label in ["first read", "second read"] {
                let read = get_policy(&harness, name).await;
                assert_eq!(
                    read.json(),
                    serde_json::json!({}),
                    "case {name} round {round} ({label}): the policy must be gone, got {}",
                    String::from_utf8_lossy(&read.body)
                );
            }

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
                is_revision_id(&granted_revision) && granted_revision != current,
                "case {name} round {round}: the re-grant must mint a fresh revision, got \
                 {granted_revision:?} after {current:?}"
            );
            assert_eq!(
                get_policy(&harness, name).await.json(),
                regranted.json(),
                "case {name} round {round}: GetPolicy must report what the re-grant \
                 stored, got {}",
                String::from_utf8_lossy(&regranted.body)
            );
            current = granted_revision;
        }
    }
}
