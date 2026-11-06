// SPDX-License-Identifier: CC0-1.0

//! The resource-based-policy surface — `PutPermission`, `GetPolicy`,
//! `RemovePermission` — end to end over HTTP, through the router this service mounts in
//! production.
//!
//! Three rules, each asserted over a table whose rows differ along what the rule
//! generalises over. None of the rows is the one the issue recorded: the group names,
//! the principal ARNs and the revisions here are this suite's own.
//!
//! 1. ABSENCE. `policy` and `revisionId` are reported exactly while a policy exists: an
//!    operation on a profiling group that RESOLVES but holds no policy — because none
//!    was ever written, or because the removal of its last statement deleted it —
//!    answers 200 with both members absent (an empty body), while a policy that still
//!    holds a statement is reported with both; and 404 `ResourceNotFoundException` is
//!    reserved for a profiling group that does not exist, for a policy the group does
//!    not have, or for a statement the existing policy does not hold.
//!    Rows: a group that was never granted, a group whose last statement was just
//!    removed, the same group granted and emptied again, and a name no group has.
//!
//! 2. RENDERING. The document a policy-returning operation reports is an IAM policy
//!    document for the stored grants: one statement per stored action group, whose `Sid`
//!    is that action group's statement id, whose `Principal.AWS` is a single principal
//!    as the ARN itself and several as a list, laid out with two-space indentation,
//!    `" : "` between a member and its value and arrays on one line. Rows: 1, 2, 3 and
//!    50 principals (the modelled `Principals` bounds are 1..50), on groups whose names
//!    span the modelled `ProfilingGroupName` length bounds, read back through both the
//!    write's own answer and `GetPolicy`; plus a policy holding two statements, one of
//!    which is then removed.
//!
//! 3. CONFLICT. A revision-checked write refused with 409 `ConflictException` states the
//!    revision the request named, where it named one, and the revision the stored policy
//!    carries, where there is one — the revision the caller retries against. Rows: a
//!    stale revision on both writes, a write that names none against a policy that
//!    exists, and a write that names one against a group that has no policy.
//!
//! The harness, the group table and the three request builders are the ones
//! [`crate::profiling_group_lifecycle`] and [`crate::permission_writes`] already export,
//! so nothing about the wiring is forked here.

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ACTION_GROUP_AGENT_PERMISSIONS, STATEMENT_SID_SUFFIX,
};

use crate::permission_writes::{get_policy, put_permission, remove_permission, string_member};
use crate::profiling_group_lifecycle::{ACCOUNT, Answer, GroupCase, Harness, group_cases};
use crate::resource_policy::attach_policy;

/// The one action group the model declares, and the `Sid` its statement carries.
const ACTION_GROUP: &str = ACTION_GROUP_AGENT_PERMISSIONS;

/// The actions `agentPermissions` grants, from that enum member's own doc: "Permission
/// group type for Agent APIs - ConfigureAgent, PostAgentProfile"
/// (`smithy_query(query="shape",shape="ActionGroup")`).
const EXPECTED_ACTIONS: &str =
    "[ \"codeguru-profiler:ConfigureAgent\", \"codeguru-profiler:PostAgentProfile\" ]";

/// A well-formed `RevisionId` no write of this store minted
/// (`smithy_query(query="shape",shape="RevisionId")`:
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`).
const FOREIGN_REVISIONS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "0e5b1ecf-1922-8afe-2c40-ef410afc120a",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "a1b2c3d4-e5f6-0789-abcd-ef0123456789",
];

/// The modelled `Principals` length bounds, which the principal-count rows span.
const MIN_PRINCIPALS: usize = 1;
const MAX_PRINCIPALS: usize = 50;

/// Principal ARNs this suite grants to: role, user, account root, service-linked role
/// path and an assumed-role session, then distinct roles past the catalogue so a row can
/// reach the modelled maximum. Source: the `principals` doc, "A list ARNs for the roles
/// and users you want to grant access to the profiling group."
fn principal(index: usize) -> String {
    let forms = [
        format!("arn:aws:iam::{ACCOUNT}:role/cgp-surface-agent"),
        format!("arn:aws:iam::{ACCOUNT}:user/cgp-surface-analyst"),
        format!("arn:aws:iam::{ACCOUNT}:root"),
        format!("arn:aws:iam::{ACCOUNT}:role/service-role/cgp-surface-lambda"),
        format!("arn:aws:sts::{ACCOUNT}:assumed-role/cgp-surface-agent/session"),
    ];
    forms
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("arn:aws:iam::{ACCOUNT}:role/cgp-surface-bulk-{index}"))
}

fn principals(count: usize) -> Vec<String> {
    (0..count).map(principal).collect()
}

/// The document a grant of `principals` on `arn` must be reported as, built from the
/// layout rule rather than from the renderer: two-space indentation, `" : "` between a
/// member and its value, arrays on one line inside `[ ` / ` ]`, one principal as the ARN
/// itself and several as a list.
fn expected_document(arn: &str, principals: &[String]) -> String {
    let principal = match principals {
        [only] => format!("\"{only}\""),
        several => format!(
            "[ {} ]",
            several
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<String>>()
                .join(", ")
        ),
    };
    let sid = format!("{ACTION_GROUP}{STATEMENT_SID_SUFFIX}");
    [
        "{".to_string(),
        "  \"Version\" : \"2012-10-17\",".to_string(),
        "  \"Statement\" : [ {".to_string(),
        format!("    \"Sid\" : \"{sid}\","),
        "    \"Effect\" : \"Allow\",".to_string(),
        "    \"Principal\" : {".to_string(),
        format!("      \"AWS\" : {principal}"),
        "    },".to_string(),
        format!("    \"Action\" : {EXPECTED_ACTIONS},"),
        format!("    \"Resource\" : \"{arn}\""),
        "  } ]".to_string(),
        "}".to_string(),
    ]
    .join("\n")
}

/// One rendering row: a group from the shared table and the principal set granted on it.
#[derive(Debug)]
struct RenderCase {
    group: GroupCase,
    principals: Vec<String>,
}

/// The rendering table: every shared group row crossed with a principal count, spanning
/// the modelled `Principals` bounds (1 and 50) and the counts between.
fn render_cases() -> Vec<RenderCase> {
    let counts = [MIN_PRINCIPALS, 2, 3, MAX_PRINCIPALS, MIN_PRINCIPALS];
    let groups = group_cases();
    assert_eq!(
        groups.len(),
        counts.len(),
        "every shared group row must carry a principal count, got {} group(s) and {} \
         count(s)",
        groups.len(),
        counts.len()
    );
    groups
        .into_iter()
        .zip(counts)
        .map(|(group, count)| RenderCase {
            group,
            principals: principals(count),
        })
        .collect()
}

/// Create every row's group, so each test starts from groups that hold no policy.
async fn created_groups(harness: &Harness, label: &str) -> Vec<GroupCase> {
    let groups = group_cases();
    for (index, group) in groups.iter().enumerate() {
        let created = harness.create(group, &format!("{label}-{index}")).await;
        assert_eq!(
            created.status,
            201,
            "case {group:?}: the group must be created first, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }
    groups
}

/// Rule 1, on an answer that reports a resource-based policy: a group that holds none is
/// a 200 whose body carries neither member.
fn assert_empty_policy_answer(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        200,
        "case {case}: a resolved group with no policy must answer 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    // The body carries NEITHER member: equality against the empty object says both
    // that nothing is reported and that nothing else crept in.
    assert_eq!(
        answer.json(),
        serde_json::json!({}),
        "case {case}: expected an empty body, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn a_group_with_no_policy_answers_an_empty_200_whether_none_was_written_or_it_was_emptied() {
    let harness = Harness::new();

    for group in created_groups(&harness, "absence").await {
        let name = group.name.as_str();

        // Row: never granted.
        assert_empty_policy_answer(
            &format!("{name}: never granted"),
            &get_policy(&harness, name).await,
        );

        // ...and a removal has nothing to remove from, which is the other 404 class:
        // the group exists, so the message names it and the account the lookup was
        // scoped to.
        for revision in FOREIGN_REVISIONS {
            let answer = remove_permission(&harness, name, ACTION_GROUP, revision).await;
            assert_eq!(
                answer.status,
                404,
                "case {name}: removing from a policy-less group must answer 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {name}: expected ResourceNotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected =
                format!("No policy attached to profiling group name: {name} in accountId: {ACCOUNT}; ");
            assert_eq!(
                answer.error_message(),
                expected,
                "case {name}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
        }

        // Rows: granted and emptied, twice over — the removal that takes the last
        // statement deletes the policy, so the second cycle starts from exactly the
        // state the first did: no policy, so the grant names no revision.
        for round in 0..2 {
            let created =
                put_permission(&harness, name, ACTION_GROUP, &principals(2), None).await;
            assert_eq!(
                created.status,
                200,
                "case {name}: round {round}: the policy must be writable, got {} {}",
                created.status,
                String::from_utf8_lossy(&created.body)
            );
            let revision = string_member(&created, "revisionId");

            let removed = remove_permission(&harness, name, ACTION_GROUP, &revision).await;
            assert_eq!(
                removed.status,
                200,
                "case {name}: round {round}: the removal must be applied, got {} {}",
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
            // The removal took the policy's only statement, so the policy is gone: the
            // removal reports neither member, and neither does the reader.
            assert_empty_policy_answer(
                &format!("{name}: round {round}: the emptying removal"),
                &removed,
            );
            assert_empty_policy_answer(
                &format!("{name}: round {round}: emptied"),
                &get_policy(&harness, name).await,
            );

            // The revision the policy used to carry names a policy that is no longer
            // there: a write naming it is refused.
            let stale =
                put_permission(&harness, name, ACTION_GROUP, &principals(1), Some(&revision))
                    .await;
            assert_eq!(
                stale.status,
                409,
                "case {name}: round {round}: a revision naming no stored policy must \
                 conflict, got {} {}",
                stale.status,
                String::from_utf8_lossy(&stale.body)
            );
        }
    }

    // The other class entirely: a name no group has. Every path answers 404 about the
    // GROUP, not about the policy — and `GetPolicy` states the lookup that found
    // nothing, naming the account it was scoped to.
    for name in ["cgp-surface-absent", "Nope_1", "z"] {
        let read = get_policy(&harness, name).await;
        let expected =
            format!("No profiling group with accountId: {ACCOUNT}; name: {name} was found.");
        assert_eq!(
            read.error_message(),
            expected,
            "case {name}: expected {expected:?}, got {:?}",
            read.error_message()
        );
        for answer in [
            read,
            remove_permission(&harness, name, ACTION_GROUP, FOREIGN_REVISIONS[0]).await,
            put_permission(&harness, name, ACTION_GROUP, &principals(1), None).await,
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {name}: a group that does not exist must answer 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {name}: expected ResourceNotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let message = answer.error_message();
            assert!(
                message.contains(name) && !message.contains("No policy attached"),
                "case {name}: a missing GROUP must be stated as a missing group, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_policy_document_is_written_the_way_a_policy_document_is_written() {
    let harness = Harness::new();
    let cases = render_cases();
    for (index, case) in cases.iter().enumerate() {
        let created = harness
            .create(&case.group, &format!("render-{index}"))
            .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the group must be created first, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    // Every group is written while the others are stored, so a row that reported
    // another row's document (or another row's ARN) fails here.
    for case in &cases {
        let name = case.group.name.as_str();
        let expected = expected_document(&case.group.expected_arn(), &case.principals);

        let written = put_permission(&harness, name, ACTION_GROUP, &case.principals, None).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the write must be applied, got {} {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        assert_eq!(
            string_member(&written, "policy"),
            expected,
            "case {case:?}: PutPermission reported a different document than the policy \
             language writes"
        );

        let read = get_policy(&harness, name).await;
        assert_eq!(
            string_member(&read, "policy"),
            expected,
            "case {case:?}: GetPolicy must report the same document the write reported"
        );
        assert_eq!(
            string_member(&read, "revisionId"),
            string_member(&written, "revisionId"),
            "case {case:?}: GetPolicy must report the stored revision"
        );
    }
}

#[tokio::test]
async fn removing_one_of_several_statements_reports_the_policy_that_is_left() {
    let harness = Harness::new();
    let group = created_groups(&harness, "multi").await.swap_remove(0);
    let name = group.name.as_str();

    // A second action group is not a value the modelled `ActionGroup` enum declares, so
    // a two-statement policy cannot be written over HTTP; it is seeded into the store
    // instead. What the row pins is the removal arm that leaves statements behind: the
    // policy is NOT cleared, so the answer carries the remaining document and the
    // revision the change minted.
    let seeded = "00000000-1111-2222-3333-444455556666";
    let kept_principals = vec![principal(3), principal(4)];
    attach_policy(
        &harness,
        name,
        &[
            (ACTION_GROUP.to_string(), principals(2)),
            ("internalAgentPermissions".to_string(), kept_principals.clone()),
        ],
        seeded,
    );

    let removed = remove_permission(&harness, name, ACTION_GROUP, seeded).await;
    assert_eq!(
        removed.status,
        200,
        "case {name}: the removal must be applied, got {} {}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );
    let revision = string_member(&removed, "revisionId");
    assert_ne!(
        revision, seeded,
        "case {name}: a removal that changed the policy must mint a new revision"
    );

    let document = string_member(&removed, "policy");
    let parsed: serde_json::Value = serde_json::from_str(&document)
        .unwrap_or_else(|error| panic!("case {name}: policy is not JSON ({error}): {document}"));
    let statements = parsed
        .get("Statement")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("case {name}: no Statement array in {document}"));
    assert_eq!(
        statements.len(),
        1,
        "case {name}: one statement was removed of two, got {document}"
    );
    assert_eq!(
        statements[0].get("Sid").and_then(|value| value.as_str()),
        Some("internalAgentPermissions-statement"),
        "case {name}: the surviving statement must be the one not removed, got {document}"
    );
    assert_eq!(
        statements[0].pointer("/Principal/AWS"),
        Some(&serde_json::Value::from(kept_principals)),
        "case {name}: two principals are written as a list, got {document}"
    );

    // The policy is still there, at the revision the removal reported.
    assert_eq!(
        string_member(&get_policy(&harness, name).await, "revisionId"),
        revision,
        "case {name}: GetPolicy must report the revision the removal minted"
    );
    assert_eq!(
        string_member(&get_policy(&harness, name).await, "policy"),
        document,
        "case {name}: GetPolicy must report the policy the removal left"
    );
}

#[tokio::test]
async fn a_revision_conflict_states_the_revision_sent_and_the_revision_to_retry_against() {
    let harness = Harness::new();

    for group in created_groups(&harness, "conflict").await {
        let name = group.name.as_str();

        // A revision named against a group that has NO policy: there is no current
        // revision to hand back, so the message states the one the request sent.
        for sent in FOREIGN_REVISIONS {
            let answer =
                put_permission(&harness, name, ACTION_GROUP, &principals(1), Some(sent)).await;
            assert_eq!(
                answer.status,
                409,
                "case {name} {sent}: expected 409, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {name} {sent}: expected ConflictException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let message = answer.error_message();
            assert!(
                message.contains(sent),
                "case {name} {sent}: the message must state the revision the request \
                 named, got {message:?}"
            );
        }

        let current = string_member(
            &put_permission(&harness, name, ACTION_GROUP, &principals(2), None).await,
            "revisionId",
        );

        // A STALE revision, on both writes: the message states the revision sent and
        // the revision to retry against.
        for sent in FOREIGN_REVISIONS {
            for (operation, answer) in [
                (
                    "PutPermission",
                    put_permission(&harness, name, ACTION_GROUP, &principals(1), Some(sent)).await,
                ),
                (
                    "RemovePermission",
                    remove_permission(&harness, name, ACTION_GROUP, sent).await,
                ),
            ] {
                assert_eq!(
                    answer.status,
                    409,
                    "case {name} {operation} {sent}: expected 409, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "ConflictException",
                    "case {name} {operation} {sent}: expected ConflictException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                let expected = format!(
                    "Policy has been updated since the revision: {sent}, new RevisionId: \
                     {current}"
                );
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {name} {operation} {sent}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
        }

        // A write that names NO revision against a policy that exists: the message
        // states the revision it must have named.
        let unversioned = put_permission(&harness, name, ACTION_GROUP, &principals(1), None).await;
        assert_eq!(
            unversioned.status,
            409,
            "case {name}: a later write naming no revision must conflict, got {} {}",
            unversioned.status,
            String::from_utf8_lossy(&unversioned.body)
        );
        let message = unversioned.error_message();
        assert!(
            message.contains(&current),
            "case {name}: the message must hand back the revision to write against, got \
             {message:?}"
        );

        // Every refusal above left the stored policy exactly where it was.
        assert_eq!(
            string_member(&get_policy(&harness, name).await, "revisionId"),
            current,
            "case {name}: a refused write must leave the stored revision alone"
        );
    }
}
