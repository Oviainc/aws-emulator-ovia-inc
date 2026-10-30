// SPDX-License-Identifier: CC0-1.0

//! What an APPLIED `RemovePermission` reports, end to end over HTTP through the router
//! this service mounts in production.
//!
//! The rule every case here asserts: an applied removal reports both members the model
//! marks `@required` on `RemovePermissionResponse` — the resource-based policy as it
//! stands after the removal (one statement per action group the policy still holds, and
//! no statement at all once the removal took the last one) and the fresh revision that
//! policy then carries, which is the revision the next write to it must name. A policy
//! exists from the write that created it until the group is deleted, so a removal never
//! answers with members a generated client would find missing.
//!
//! Rows differ along what the rule generalises over: how many statements the policy
//! holds when the removal arrives (1, 2 and 3 — so the removal leaves 0, 1 and 2
//! behind), how many principals the removed statement granted to (1, 2 and 50, the
//! modelled `Principals` bounds being 1..50), the ARN form of those principals, and the
//! profiling group the policy sits on (the shared group table spans the modelled
//! `ProfilingGroupName` length bounds). No row is privileged; every row runs through the
//! same assertions.
//!
//! A policy holding more than one statement cannot be written over HTTP — the modelled
//! `ActionGroup` enum declares one value — so those rows are seeded into the store
//! through the state writers [`crate::resource_policy::attach_policy`] already uses, the
//! same way the read suite seeds a multi-statement document. Every row's REMOVAL and
//! every read-back goes over HTTP.

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ACTION_GROUP_AGENT_PERMISSIONS, POLICY_VERSION, action_group_actions,
    action_group_statement_id,
};

use crate::permission_writes::{
    get_policy, is_revision_id, put_permission, remove_permission, string_member,
};
use crate::profiling_group_lifecycle::{ACCOUNT, Answer, GroupCase, Harness, group_cases};
use crate::resource_policy::attach_policy;

/// The action group every row removes: the one value the modelled `ActionGroup` enum
/// declares, which is also the only one a policy write admits.
const REMOVED_ACTION_GROUP: &str = ACTION_GROUP_AGENT_PERMISSIONS;

/// Action groups a policy can be SEEDED with beside the removed one, so a row can hold
/// more than one statement. These are not modelled enum values — they cannot be written
/// over HTTP — and what they contribute is a statement the removal must leave alone.
const COMPANION_ACTION_GROUPS: &[&str] = &["internalAgentPermissions", "consolePermissions"];

/// Principal ARNs a seeded grant names: the forms the `principals` doc names ("A list
/// ARNs for the roles and users you want to grant access to the profiling group"), then
/// distinct role ARNs past that catalogue so a row can reach the modelled maximum. The
/// account id is the emulator's own.
fn principal(index: usize) -> String {
    let forms = [
        format!("arn:aws:iam::{ACCOUNT}:role/cgp-removal-agent"),
        format!("arn:aws:iam::{ACCOUNT}:user/cgp-removal-analyst"),
        format!("arn:aws:iam::{ACCOUNT}:root"),
        format!("arn:aws:iam::{ACCOUNT}:role/service-role/cgp-removal-lambda"),
        format!("arn:aws:sts::{ACCOUNT}:assumed-role/cgp-removal-agent/session"),
    ];
    forms
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("arn:aws:iam::{ACCOUNT}:role/cgp-removal-bulk-{index}"))
}

fn principals(count: usize) -> Vec<String> {
    (0..count).map(principal).collect()
}

/// One row: the group the policy sits on, the statements the policy holds when the
/// removal arrives (the removed action group first), and the revision it is seeded at.
#[derive(Debug)]
struct RemovalCase {
    group: GroupCase,
    /// `actionGroup` -> the principals it grants to. The first entry is the statement
    /// the row removes; the rest are the statements it must leave behind.
    held: Vec<(String, Vec<String>)>,
    /// The revision the seeded policy carries, in the modelled `RevisionId` form.
    revision_id: String,
}

impl RemovalCase {
    /// The action groups whose statements must survive the removal.
    fn kept(&self) -> Vec<&(String, Vec<String>)> {
        self.held.iter().skip(1).collect()
    }
}

/// The table: every row of the shared group table, crossed with a statement count (1, 2,
/// 3, 1 and 2) and a principal count spanning the modelled `Principals` bounds (1 and 50)
/// and the counts between.
fn removal_cases() -> Vec<RemovalCase> {
    // (how many statements the policy holds, how many principals the removed one grants
    // to, the revision the policy is seeded at).
    let shapes: [(usize, usize, &str); 5] = [
        (1, 1, "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d"),
        (2, 2, "ffffffff-0000-1111-2222-333344445555"),
        (3, 50, "12345678-90ab-cdef-0123-456789abcdef"),
        (1, 50, "abcdef01-2345-6789-abcd-ef0123456789"),
        (2, 3, "deadbeef-dead-beef-dead-beefdeadbeef"),
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
            RemovalCase {
                group,
                held,
                revision_id: revision_id.to_string(),
            }
        })
        .collect()
}

/// The statements `answer`'s `policy` member carries, keyed by `Sid`.
fn statements_by_sid(
    case: &RemovalCase,
    label: &str,
    answer: &Answer,
) -> Vec<(String, serde_json::Value)> {
    let document = string_member(answer, "policy");
    let parsed: serde_json::Value = serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("case {case:?} ({label}): policy is not a JSON document ({error}): {document}")
    });
    assert_eq!(
        parsed.get("Version").and_then(serde_json::Value::as_str),
        Some(POLICY_VERSION),
        "case {case:?} ({label}): wrong policy Version in {document}"
    );
    parsed
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?} ({label}): no Statement array in {document}"))
        .iter()
        .map(|statement| {
            (
                statement
                    .get("Sid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?} ({label}): a statement with no Sid in {document}")
                    })
                    .to_string(),
                statement.clone(),
            )
        })
        .collect()
}

/// Assert `answer` reports exactly the statements the row keeps, and nothing of the one
/// it removed.
fn assert_reports_the_remaining_policy(case: &RemovalCase, label: &str, answer: &Answer) {
    let statements = statements_by_sid(case, label, answer);
    let kept = case.kept();
    assert_eq!(
        statements.len(),
        kept.len(),
        "case {case:?} ({label}): expected {} statement(s) left, got {statements:?}",
        kept.len()
    );
    let removed_sid = action_group_statement_id(REMOVED_ACTION_GROUP);
    assert!(
        !statements.iter().any(|(sid, _)| *sid == removed_sid),
        "case {case:?} ({label}): the removed action group must hold no statement, got \
         {statements:?}"
    );
    for (action_group, granted) in kept {
        let sid = action_group_statement_id(action_group);
        let (_, statement) = statements
            .iter()
            .find(|(reported, _)| *reported == sid)
            .unwrap_or_else(|| {
                panic!("case {case:?} ({label}): no statement for {sid} in {statements:?}")
            });
        assert_eq!(
            statement.get("Effect").and_then(serde_json::Value::as_str),
            Some("Allow"),
            "case {case:?} ({label}): wrong Effect in {statement}"
        );
        // One principal is written as the ARN itself, several as a list.
        let expected_principal = match granted.as_slice() {
            [only] => serde_json::Value::from(only.clone()),
            several => serde_json::Value::from(several.to_vec()),
        };
        assert_eq!(
            statement.pointer("/Principal/AWS"),
            Some(&expected_principal),
            "case {case:?} ({label}): wrong Principal.AWS in {statement}"
        );
        assert_eq!(
            statement.get("Action"),
            Some(&serde_json::Value::from(action_group_actions(action_group))),
            "case {case:?} ({label}): wrong Action in {statement}"
        );
        assert_eq!(
            statement.get("Resource").and_then(serde_json::Value::as_str),
            Some(case.group.expected_arn().as_str()),
            "case {case:?} ({label}): the statement's Resource must be the group's own \
             ARN, got {statement}"
        );
    }
}

/// Create every row's group and seed its policy, so every row is stored at once: a row
/// that reported another row's document fails here rather than being hidden by a fresh
/// store.
async fn seeded_cases(harness: &Harness) -> Vec<RemovalCase> {
    let cases = removal_cases();
    for (index, case) in cases.iter().enumerate() {
        let created = harness
            .create(&case.group, &format!("removal-tok-{index}"))
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

#[tokio::test]
async fn an_applied_removal_reports_the_policy_and_revision_it_leaves_behind() {
    let harness = Harness::new();

    for case in seeded_cases(&harness).await {
        let name = case.group.name.as_str();
        let removed =
            remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id).await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: a removal naming the stored revision must be applied, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );

        // BOTH members the model marks `@required` are in the body, and nothing else is.
        let body = removed.json();
        let reported: Vec<&String> = body
            .as_object()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the answer must be a JSON object, got {}",
                    String::from_utf8_lossy(&removed.body)
                )
            })
            .keys()
            .collect();
        assert_eq!(
            reported,
            vec!["policy", "revisionId"],
            "case {case:?}: a removal must report the policy and the revision, got {}",
            String::from_utf8_lossy(&removed.body)
        );

        let revision = string_member(&removed, "revisionId");
        assert!(
            is_revision_id(&revision),
            "case {case:?}: expected a revisionId of the modelled form, got {revision:?}"
        );
        assert_ne!(
            revision, case.revision_id,
            "case {case:?}: an applied removal must mint a new revision, got {revision}"
        );
        assert_reports_the_remaining_policy(&case, "the removal", &removed);

        // The same policy is what the group holds now, so the reader reports exactly
        // what the removal reported.
        let read = get_policy(&harness, name).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetPolicy must answer 200, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json(),
            body,
            "case {case:?}: GetPolicy must report the policy and revision the removal \
             left, got {}",
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            harness.stored_policy_revision(name),
            Some(revision.clone()),
            "case {case:?}: the stored policy must carry the revision the removal \
             reported, got {:?}",
            harness.stored_policy_revision(name)
        );
    }
}

#[tokio::test]
async fn the_revision_a_removal_reports_is_the_one_the_next_write_must_name() {
    let harness = Harness::new();

    for case in seeded_cases(&harness).await {
        let name = case.group.name.as_str();
        let removed =
            remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id).await;
        let revision = string_member(&removed, "revisionId");

        // The policy has moved past the revision it was seeded at, so a write naming
        // that one is a conflict...
        let superseded =
            remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id).await;
        assert_eq!(
            superseded.status,
            409,
            "case {case:?}: a superseded revision must conflict, got {} {}",
            superseded.status,
            String::from_utf8_lossy(&superseded.body)
        );
        assert_eq!(
            superseded.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&superseded.body)
        );

        // ...while the revision the removal reported passes the revision check: the
        // removal of a statement the policy no longer holds is refused for THAT reason,
        // as the operation's 404 `ResourceNotFoundException`.
        let again = remove_permission(&harness, name, REMOVED_ACTION_GROUP, &revision).await;
        assert_eq!(
            again.status,
            404,
            "case {case:?}: removing a statement the policy does not hold must answer \
             404, got {} {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&again.body)
        );

        // A policy exists, so the next grant must name its revision — and the one the
        // removal reported is the one it must name. Rows the removal left statements on
        // are already at the one-statement-per-action-group bound the store holds to, so
        // only the rows it emptied can take the grant; the refusal is checked for every
        // row, since it follows from the policy existing.
        let unversioned =
            put_permission(&harness, name, REMOVED_ACTION_GROUP, &principals(1), None).await;
        assert_eq!(
            unversioned.status,
            409,
            "case {case:?}: a write against an existing policy must name its revision, \
             got {} {}",
            unversioned.status,
            String::from_utf8_lossy(&unversioned.body)
        );
        if !case.kept().is_empty() {
            continue;
        }
        let regranted = put_permission(
            &harness,
            name,
            REMOVED_ACTION_GROUP,
            &principals(2),
            Some(&revision),
        )
        .await;
        assert_eq!(
            regranted.status,
            200,
            "case {case:?}: the revision the removal reported must be the one a later \
             write names, got {} {}",
            regranted.status,
            String::from_utf8_lossy(&regranted.body)
        );
        // Round trip: the regrant reads back through the reader, at the revision the
        // regrant minted.
        let read = get_policy(&harness, name).await;
        assert_eq!(
            read.json(),
            regranted.json(),
            "case {case:?}: GetPolicy must report what the regrant stored, got {}",
            String::from_utf8_lossy(&read.body)
        );
        let regrant_revision = string_member(&regranted, "revisionId");
        assert!(
            is_revision_id(&regrant_revision) && regrant_revision != revision,
            "case {case:?}: the regrant must mint a fresh revision, got \
             {regrant_revision:?} after {revision:?}"
        );
    }
}

#[tokio::test]
async fn a_removal_that_empties_the_policy_reports_a_document_with_no_statement() {
    let harness = Harness::new();

    // The rows the rule's boundary sits on: a policy holding ONE statement, so the
    // removal leaves a policy with no statement at all. The document is built from the
    // layout rule the policy surface writes documents in (two-space indentation, `" : "`
    // between a member and its value, an array on one line inside `[ ` / ` ]`), not from
    // the renderer.
    let expected = format!("{{\n  \"Version\" : \"{POLICY_VERSION}\",\n  \"Statement\" : [ ]\n}}");

    for case in seeded_cases(&harness)
        .await
        .into_iter()
        .filter(|case| case.held.len() == 1)
    {
        let name = case.group.name.as_str();
        let removed =
            remove_permission(&harness, name, REMOVED_ACTION_GROUP, &case.revision_id).await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: the removal must be applied, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        assert_eq!(
            string_member(&removed, "policy"),
            expected,
            "case {case:?}: a policy with no statement must be reported as the document \
             with an empty Statement array"
        );
        for granted in &case.held[0].1 {
            assert!(
                !string_member(&removed, "policy").contains(granted),
                "case {case:?}: {granted} was removed, so it must be granted nothing in {}",
                string_member(&removed, "policy")
            );
        }
        assert_eq!(
            string_member(&get_policy(&harness, name).await, "policy"),
            expected,
            "case {case:?}: GetPolicy must report the statement-less document too"
        );
    }
}
