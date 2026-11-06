// SPDX-License-Identifier: CC0-1.0

//! WHICH members a resource-policy answer serializes, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule: a profiling group holds a resource-based policy exactly while that policy
//! holds at least one statement, and an operation that reports a policy
//! (`PutPermission`, `RemovePermission`, `GetPolicy`) serializes `policy` and
//! `revisionId` only when the group holds one once the call is done — so the removal
//! that takes the LAST statement deletes the policy and answers 200 carrying NEITHER
//! member, exactly as `GetPolicy` then does on the same group, while a removal that
//! leaves a statement behind reports the document it left and the revision it minted.
//! A policy that does not exist is at no revision, so a deleting removal mints none
//! either: the store's revision counter does not move for it.
//!
//! What this suite adds to the sibling [`crate::last_statement_removal_deletes_the_policy`],
//! whose table and seeding it SHARES rather than forking: the member SET each answer
//! carries (so a member that crept in fails here), the equality of the reader's body
//! with the removal's, and the revision the store minted — or did not — for the write.
//! The state the deletion leaves behind, the refusals it makes due and the first write
//! that follows are that sibling's; the document a surviving policy renders is
//! [`crate::removal_reports_the_policy`]'s.
//!
//! The shared rows differ along what the rule generalises over: how many statements the
//! policy holds when the removal arrives (1, 2 and 3 — so the removal leaves 0, 1 and 2
//! behind), how many principals the removed statement granted to (1, 2 and 50, the
//! modelled `Principals` bounds being 1..50), and the profiling group it sits on (the
//! shared group table spans the modelled `ProfilingGroupName` length bounds 1..255).

use crate::last_statement_removal_deletes_the_policy::{
    REMOVED_ACTION_GROUP, principals, seeded, statement_count,
};
use crate::minted_ids_follow_applied_writes::mint_counts;
use crate::permission_writes::{
    get_policy, is_revision_id, put_permission, remove_permission, string_member,
};
use crate::profiling_group_lifecycle::{Answer, Harness};

/// The two members a resource-policy answer can carry, in the model's member order
/// (`smithy_query(query="shape",shape="RemovePermissionResponse,GetPolicyResponse")`).
const POLICY_MEMBERS: [&str; 2] = ["policy", "revisionId"];

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

/// The members an answer must carry for a group that holds a policy: both of them.
fn both_members() -> Vec<String> {
    POLICY_MEMBERS
        .iter()
        .map(|member| (*member).to_string())
        .collect()
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
            both_members(),
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
        // Which members the body carries is the policy's existence, and nothing else may
        // be in it either way.
        let expected_members = if case.statements_left() == 0 {
            Vec::new()
        } else {
            both_members()
        };
        assert_eq!(
            reported_members(&label, &removed),
            expected_members,
            "case {label}: expected the members {expected_members:?}, got {}",
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
            // The policy is GONE: nothing is reported of it, and there is no policy to
            // be at a revision, so none was minted.
            assert_eq!(
                read.json(),
                serde_json::json!({}),
                "case {label}: a removal that took the last statement must report \
                 neither member, got {}",
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                minted_after, minted_before,
                "case {label}: a removal that deleted the policy must mint no revision; \
                 the store's (channelIds, revisions) counters moved from \
                 {minted_before:?} to {minted_after:?}"
            );

            // ...and the counter it left alone is the one the next APPLIED write draws
            // from: the group is policy-less, so that write is a first grant again, and
            // it mints exactly one revision, which reads back through the reader.
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
                mint_counts(&harness),
                (minted_after.0, minted_after.1 + 1),
                "case {label}: an applied grant must mint exactly one revision, got {:?} \
                 from {minted_after:?}",
                mint_counts(&harness)
            );
            assert_eq!(
                get_policy(&harness, name).await.json(),
                regranted.json(),
                "case {label}: GetPolicy must report what the re-grant stored, got {}",
                String::from_utf8_lossy(&regranted.body)
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
