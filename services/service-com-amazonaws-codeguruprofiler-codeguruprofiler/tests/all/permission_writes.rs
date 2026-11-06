// SPDX-License-Identifier: CC0-1.0

//! `PutPermission` and `RemovePermission` end to end over HTTP, through the router this
//! service mounts in production — the WRITES to a profiling group's resource-based
//! policy, read back through `GetPolicy`.
//!
//! The rule every case here asserts: a profiling group holds ONE resource-based policy,
//! and every answer is computed from it — `PutPermission` writes an action group's
//! statement, REPLACING (never merging) the principal set it grants to and minting a
//! fresh `revisionId`; a write is applied only when it names the revision the stored
//! policy currently carries (and, for the first write, names none); `GetPolicy` reports
//! that stored document and revision unchanged; and `RemovePermission` drops the
//! statement and reports the policy as it then stands with the revision it then carries.
//! A policy exists only while it holds a statement, so a removal that took the LAST one
//! deletes it: that removal's own 200 carries neither member, `GetPolicy` then reports
//! neither either, and the next grant is a first write again, naming no revision.
//!
//! The harness and the group table are the ones [`crate::profiling_group_lifecycle`]
//! already builds, so the group variation (compute platform, orchestration flag, tag
//! count, name length 1..255) comes for free; each row is crossed with a principal set
//! of its own, which is what this rule generalises over: how many principals a write
//! grants to (1, the modelled minimum, through 50, the modelled maximum), their form
//! (role, user, account root, service-linked role, assumed-role session), whether the
//! list repeats one, and whether the next write grows or shrinks the set. No row is
//! privileged: the whole table is driven through the same assertions.
//!
//! The READ side of the same stored policy — `GetPolicy` over seeded state, several
//! action groups in one document, and a deleted group — is [`crate::resource_policy`].

use crate::profiling_group_lifecycle::{Answer, GroupCase, Harness, group_cases};

/// The only value the modelled `ActionGroup` enum declares
/// (`smithy_query(query="shape",shape="ActionGroup")`).
const ACTION_GROUP: &str = "agentPermissions";

/// The `Sid` the statement for [`ACTION_GROUP`] carries: the action group's statement
/// id, which is its name plus the `-statement` suffix every statement id carries.
const ACTION_GROUP_SID: &str = "agentPermissions-statement";

/// The permissions `agentPermissions` grants, from that enum member's own doc:
/// "Permission group type for Agent APIs - ConfigureAgent, PostAgentProfile"
/// (`smithy_query(query="shape",shape="ActionGroup")`).
const EXPECTED_ACTIONS: &[&str] = &[
    "codeguru-profiler:ConfigureAgent",
    "codeguru-profiler:PostAgentProfile",
];

/// The modelled `Principals` length bounds
/// (`smithy_query(query="shape",shape="Principals")`: `length 1..50`).
const MIN_PRINCIPALS: usize = 1;
const MAX_PRINCIPALS: usize = 50;

/// A well-formed `RevisionId` that is not one this service minted, for the cases that
/// name a revision the store does not hold.
const FOREIGN_REVISION: &str = "00000000-0000-4000-8000-000000000000";

/// A string member of a JSON answer.
pub(crate) fn string_member(answer: &Answer, member: &str) -> String {
    answer
        .json()
        .get(member)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no string member {member} in {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .to_string()
}

/// The `policy` member, parsed as the JSON document it carries.
fn policy_document(answer: &Answer) -> serde_json::Value {
    let policy = string_member(answer, "policy");
    serde_json::from_str(&policy)
        .unwrap_or_else(|error| panic!("policy must be a JSON document, got {error}: {policy}"))
}

/// `PutPermission`: `PUT /profilingGroups/{profilingGroupName}/policy/{actionGroup}`,
/// with `principals` and the optional `revisionId` in the payload.
pub(crate) async fn put_permission(
    harness: &Harness,
    group: &str,
    action_group: &str,
    principals: &[String],
    revision_id: Option<&str>,
) -> Answer {
    let mut body = serde_json::Map::new();
    body.insert(
        "principals".to_string(),
        serde_json::Value::Array(
            principals
                .iter()
                .map(|principal| serde_json::Value::from(principal.clone()))
                .collect(),
        ),
    );
    if let Some(revision) = revision_id {
        body.insert("revisionId".to_string(), serde_json::Value::from(revision));
    }
    harness
        .send(
            "PUT",
            &format!("/profilingGroups/{group}/policy/{action_group}"),
            "application/json",
            serde_json::to_vec(&serde_json::Value::Object(body)).expect("a serializable body"),
        )
        .await
}

/// `GetPolicy`: `GET /profilingGroups/{profilingGroupName}/policy`.
pub(crate) async fn get_policy(harness: &Harness, group: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/profilingGroups/{group}/policy"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// `RemovePermission`:
/// `DELETE /profilingGroups/{profilingGroupName}/policy/{actionGroup}?revisionId=..`,
/// whose `revisionId` is modelled as a required query parameter.
pub(crate) async fn remove_permission(
    harness: &Harness,
    group: &str,
    action_group: &str,
    revision_id: &str,
) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{group}/policy/{action_group}?revisionId={revision_id}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// A principal catalogue: the ARN forms a resource-based policy grants to. Source: the
/// `principals` doc, "A list ARNs for the roles and users you want to grant access to
/// the profiling group. Wildcards are not are supported in the ARNs."
/// (`smithy_query(query="operation",operation="PutPermission")`). The account id is the
/// emulator's own; none of these is the ARN the issue's recording used.
fn principal(index: usize) -> String {
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    let forms = [
        format!("arn:aws:iam::{account}:role/AgentRole"),
        format!("arn:aws:iam::{account}:user/profiler-user"),
        format!("arn:aws:iam::{account}:root"),
        format!("arn:aws:iam::{account}:role/service-role/lambda-profiled"),
        format!("arn:aws:sts::{account}:assumed-role/AgentRole/session"),
    ];
    match forms.get(index) {
        Some(form) => form.clone(),
        // Past the catalogue, keep minting distinct role ARNs, so a row can span the
        // modelled list length.
        None => format!("arn:aws:iam::{account}:role/Agent{index}"),
    }
}

fn principals(count: usize) -> Vec<String> {
    (0..count).map(principal).collect()
}

/// One row: a group from the shared table, the principal set its first write grants
/// to, and the set the second write replaces that with.
#[derive(Debug)]
struct WriteCase {
    group: GroupCase,
    first: Vec<String>,
    second: Vec<String>,
}

/// The table: every group the shared `group_cases()` table spans, each crossed with a
/// principal set of its own — the modelled minimum count, a few, a list that repeats
/// one principal (a grant is a SET), and the modelled maximum — and a second write
/// that grows the set for some rows and shrinks it for others.
fn write_cases() -> Vec<WriteCase> {
    group_cases()
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let (first, second) = match index % 5 {
                0 => (principals(MIN_PRINCIPALS), principals(3)),
                1 => (principals(5), principals(2)),
                2 => (
                    vec![principal(1), principal(1), principal(0)],
                    vec![principal(2)],
                ),
                3 => (principals(MAX_PRINCIPALS), principals(MIN_PRINCIPALS)),
                _ => (vec![principal(4)], principals(MAX_PRINCIPALS)),
            };
            WriteCase {
                group,
                first,
                second,
            }
        })
        .collect()
}

/// The principals a grant of `requested` must report: the request's order, each
/// principal once.
fn expected_principals(requested: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for principal in requested {
        if !unique.contains(principal) {
            unique.push(principal.clone());
        }
    }
    unique
}

/// `true` when `value` has the form the modelled `RevisionId` pattern fixes,
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`
/// (`smithy_query(query="shape",shape="RevisionId")`). `pub(crate)` so the suites that
/// check a reported revision against the model read the pattern from one place.
pub(crate) fn is_revision_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8_usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// Assert the policy document grants exactly `granted` on `case`'s group, with the
/// action group's own actions and that group's own ARN as the resource.
fn assert_policy_grants(case: &WriteCase, policy: &serde_json::Value, granted: &[String]) {
    assert_eq!(
        policy.get("Version").and_then(|v| v.as_str()),
        Some("2012-10-17"),
        "case {case:?}: the policy document must carry the IAM policy version, got {policy}"
    );
    let statements = policy
        .get("Statement")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("case {case:?}: no Statement array in {policy}"));
    assert_eq!(
        statements.len(),
        1,
        "case {case:?}: expected one statement, got {policy}"
    );
    let statement = &statements[0];
    assert_eq!(
        statement.get("Sid").and_then(|v| v.as_str()),
        Some(ACTION_GROUP_SID),
        "case {case:?}: the statement's Sid must be the action group's statement id, got {statement}"
    );
    assert_eq!(
        statement.get("Effect").and_then(|v| v.as_str()),
        Some("Allow"),
        "case {case:?}: a granted action group must Allow, got {statement}"
    );
    // One principal is written as the ARN itself, several as a list.
    let expected_principal = match granted {
        [only] => serde_json::Value::from(only.clone()),
        several => serde_json::Value::from(several.to_vec()),
    };
    assert_eq!(
        statement.pointer("/Principal/AWS"),
        Some(&expected_principal),
        "case {case:?}: expected the stored principal set {granted:?} in the policy \
         language's own form, got {statement}"
    );
    let actions: Vec<String> = statement
        .get("Action")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("case {case:?}: no Action array in {statement}"))
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        actions, EXPECTED_ACTIONS,
        "case {case:?}: expected the action group's permissions {EXPECTED_ACTIONS:?}, got {actions:?}"
    );
    assert_eq!(
        statement.get("Resource").and_then(|v| v.as_str()),
        Some(case.group.expected_arn().as_str()),
        "case {case:?}: the statement's Resource must be the profiling group's own ARN, got {statement}"
    );
}

/// Create every row's group, so each test starts from a store that holds the groups
/// and no policy at all.
async fn created_cases(harness: &Harness) -> Vec<WriteCase> {
    let cases = write_cases();
    for (index, case) in cases.iter().enumerate() {
        let created = harness
            .create(&case.group, &format!("write-tok-{index}"))
            .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the group must be created first, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }
    cases
}

#[tokio::test]
async fn a_put_permission_grant_reads_back_through_get_policy() {
    let harness = Harness::new();

    for case in created_cases(&harness).await {
        let group = case.group.name.as_str();
        let unshared = get_policy(&harness, group).await;
        assert_eq!(
            unshared.status,
            200,
            "case {case:?}: a group with no resource-based policy must answer 200, got {} {}",
            unshared.status,
            String::from_utf8_lossy(&unshared.body)
        );
        assert_eq!(
            unshared.json(),
            serde_json::json!({}),
            "case {case:?}: a group with no resource-based policy must answer an empty \
             body, got {}",
            String::from_utf8_lossy(&unshared.body)
        );

        let created = put_permission(&harness, group, ACTION_GROUP, &case.first, None).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the first write must create the policy, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let revision = string_member(&created, "revisionId");
        assert!(
            is_revision_id(&revision),
            "case {case:?}: expected a UUID revisionId, got {revision:?}"
        );
        assert_policy_grants(
            &case,
            &policy_document(&created),
            &expected_principals(&case.first),
        );

        // The read reports the stored document and revision, byte for byte.
        let read = get_policy(&harness, group).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetPolicy must answer 200, got {}",
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            string_member(&read, "policy"),
            string_member(&created, "policy"),
            "case {case:?}: GetPolicy must report the stored policy document"
        );
        assert_eq!(
            string_member(&read, "revisionId"),
            revision,
            "case {case:?}: GetPolicy must report the stored revision"
        );
        // ...and reading it changes nothing a later caller sees.
        let reread = get_policy(&harness, group).await;
        assert_eq!(
            string_member(&reread, "revisionId"),
            revision,
            "case {case:?}: a read must not move the policy's revision"
        );
    }
}

#[tokio::test]
async fn a_second_put_permission_replaces_the_principal_set_and_mints_a_new_revision() {
    let harness = Harness::new();

    for case in created_cases(&harness).await {
        let group = case.group.name.as_str();
        let first = put_permission(&harness, group, ACTION_GROUP, &case.first, None).await;
        let first_revision = string_member(&first, "revisionId");

        let second = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &case.second,
            Some(&first_revision),
        )
        .await;
        assert_eq!(
            second.status,
            200,
            "case {case:?}: a write naming the current revision must be applied, got {} {}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let second_revision = string_member(&second, "revisionId");
        assert!(
            is_revision_id(&second_revision),
            "case {case:?}: expected a UUID revisionId, got {second_revision:?}"
        );
        assert_ne!(
            second_revision, first_revision,
            "case {case:?}: an update must mint a new revision, got {second_revision}"
        );

        // Replaced, not merged: only the second write's principals are granted — and
        // every principal the first write granted that the second did not is gone.
        let expected = expected_principals(&case.second);
        assert_policy_grants(&case, &policy_document(&second), &expected);
        let document = string_member(&second, "policy");
        for dropped in expected_principals(&case.first)
            .into_iter()
            .filter(|principal| !expected.contains(principal))
        {
            assert!(
                !document.contains(&dropped),
                "case {case:?}: {dropped} was replaced, so it must not still be granted in {document}"
            );
        }
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "policy"),
            document,
            "case {case:?}: GetPolicy must report the replacing document"
        );

        // A third write naming the revision the second one superseded is stale.
        let stale = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &case.first,
            Some(&first_revision),
        )
        .await;
        assert_eq!(
            stale.status,
            409,
            "case {case:?}: a stale revision must conflict, got {} {}",
            stale.status,
            String::from_utf8_lossy(&stale.body)
        );
        assert_eq!(
            stale.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&stale.body)
        );
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "revisionId"),
            second_revision,
            "case {case:?}: a refused write must leave the stored policy untouched"
        );
    }
}

#[tokio::test]
async fn the_first_write_must_name_no_revision_and_every_later_write_must_name_the_current_one() {
    let harness = Harness::new();

    for case in created_cases(&harness).await {
        let group = case.group.name.as_str();

        // No policy yet, so naming a revision has nothing to match.
        let premature = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &case.first,
            Some(FOREIGN_REVISION),
        )
        .await;
        assert_eq!(
            premature.status,
            409,
            "case {case:?}: naming a revision before a policy exists must conflict, got {} {}",
            premature.status,
            String::from_utf8_lossy(&premature.body)
        );
        assert_eq!(
            premature.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&premature.body)
        );
        assert_eq!(
            harness.stored_policy_revision(group),
            None,
            "case {case:?}: a refused first write must create no policy"
        );

        let created = put_permission(&harness, group, ACTION_GROUP, &case.first, None).await;
        let revision = string_member(&created, "revisionId");

        // A policy exists, so a write that names none has not said which revision it
        // is updating.
        let unversioned = put_permission(&harness, group, ACTION_GROUP, &case.second, None).await;
        assert_eq!(
            unversioned.status,
            409,
            "case {case:?}: a later write with no revisionId must conflict, got {} {}",
            unversioned.status,
            String::from_utf8_lossy(&unversioned.body)
        );
        assert_eq!(
            harness.stored_policy_revision(group),
            Some(revision.clone()),
            "case {case:?}: a refused write must leave the revision where it was"
        );

        // A revision that is well-formed but is not the stored one.
        let wrong = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &case.second,
            Some("ffffffff-ffff-4fff-bfff-ffffffffffff"),
        )
        .await;
        assert_eq!(
            wrong.status,
            409,
            "case {case:?}: a revision other than the current one must conflict, got {} {}",
            wrong.status,
            String::from_utf8_lossy(&wrong.body)
        );
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "policy"),
            string_member(&created, "policy"),
            "case {case:?}: a refused write must leave the document untouched"
        );
    }
}

#[tokio::test]
async fn remove_permission_drops_the_statement_and_reports_the_policy_it_leaves_behind() {
    let harness = Harness::new();

    for case in created_cases(&harness).await {
        let group = case.group.name.as_str();
        let created = put_permission(&harness, group, ACTION_GROUP, &case.first, None).await;
        let revision = string_member(&created, "revisionId");

        // A removal naming a revision the policy does not carry is refused, and
        // nothing is removed.
        let stale = remove_permission(&harness, group, ACTION_GROUP, FOREIGN_REVISION).await;
        assert_eq!(
            stale.status,
            409,
            "case {case:?}: a stale revision must conflict, got {} {}",
            stale.status,
            String::from_utf8_lossy(&stale.body)
        );
        assert_eq!(
            stale.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&stale.body)
        );
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "policy"),
            string_member(&created, "policy"),
            "case {case:?}: a refused removal must leave the policy in place"
        );

        let removed = remove_permission(&harness, group, ACTION_GROUP, &revision).await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: a removal naming the current revision must be applied, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        // The removal took the policy's only statement, so the policy is GONE: a policy
        // exists only while it holds a statement, so there is no document to report and
        // no revision to report it at, and the answer carries neither member — nor,
        // therefore, anything the policy granted.
        assert_eq!(
            removed.json(),
            serde_json::json!({}),
            "case {case:?}: a removal that deleted the policy must report neither policy \
             nor revisionId, got {}",
            String::from_utf8_lossy(&removed.body)
        );
        for granted in expected_principals(&case.first) {
            assert!(
                !String::from_utf8_lossy(&removed.body).contains(&granted),
                "case {case:?}: {granted} was removed, so it must not still be granted in {}",
                String::from_utf8_lossy(&removed.body)
            );
        }

        // The group stores no policy either, and `GetPolicy` reports neither member.
        assert_eq!(
            harness.stored_policy_revision(group),
            None,
            "case {case:?}: the removal took the last statement, so the group must store \
             no policy, got {:?}",
            harness.stored_policy_revision(group)
        );
        let read = get_policy(&harness, group).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the group still exists, so GetPolicy must answer 200, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json(),
            serde_json::json!({}),
            "case {case:?}: a group whose policy was deleted must report neither policy \
             nor revisionId, got {}",
            String::from_utf8_lossy(&read.body)
        );

        // There is no policy, so the revision the removal consumed names none either:
        // a repeat removal is refused about the policy.
        let again = remove_permission(&harness, group, ACTION_GROUP, &revision).await;
        assert_eq!(
            again.status,
            404,
            "case {case:?}: a removal at {revision} must find no policy, got {} {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&again.body)
        );

        // The group has no policy, so the next grant is a FIRST write: it names no
        // revision, and one that names the consumed revision is refused.
        let stale_grant =
            put_permission(&harness, group, ACTION_GROUP, &case.second, Some(&revision)).await;
        assert_eq!(
            stale_grant.status,
            409,
            "case {case:?}: naming a revision when no policy exists must conflict, got {} {}",
            stale_grant.status,
            String::from_utf8_lossy(&stale_grant.body)
        );
        let regranted = put_permission(&harness, group, ACTION_GROUP, &case.second, None).await;
        assert_eq!(
            regranted.status,
            200,
            "case {case:?}: a group with no policy must take a write that names no \
             revision, got {} {}",
            regranted.status,
            String::from_utf8_lossy(&regranted.body)
        );
        assert_policy_grants(
            &case,
            &policy_document(&regranted),
            &expected_principals(&case.second),
        );
    }
}

#[tokio::test]
async fn two_writes_racing_on_one_revision_leave_exactly_one_winner() {
    let harness = Harness::new();

    for case in created_cases(&harness).await {
        let group = case.group.name.as_str();
        let revision = string_member(
            &put_permission(&harness, group, ACTION_GROUP, &case.first, None).await,
            "revisionId",
        );

        // Both writes hold the same revision, so the policy's revision is the lock:
        // whichever is applied first invalidates the other.
        let (left, right) = tokio::join!(
            put_permission(&harness, group, ACTION_GROUP, &case.second, Some(&revision)),
            put_permission(&harness, group, ACTION_GROUP, &case.first, Some(&revision)),
        );
        let mut statuses = [left.status, right.status];
        statuses.sort_unstable();
        assert_eq!(
            statuses,
            [200, 409],
            "case {case:?}: exactly one racing write must win, got {statuses:?}: {} / {}",
            String::from_utf8_lossy(&left.body),
            String::from_utf8_lossy(&right.body)
        );

        let winner = if left.status == 200 { &left } else { &right };
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "revisionId"),
            string_member(winner, "revisionId"),
            "case {case:?}: the stored revision must be the winner's"
        );
        assert_eq!(
            string_member(&get_policy(&harness, group).await, "policy"),
            string_member(winner, "policy"),
            "case {case:?}: the stored document must be the winner's"
        );
    }
}

#[tokio::test]
async fn a_policy_write_is_refused_outside_the_modelled_input_and_on_an_unknown_group() {
    let harness = Harness::new();
    let cases = created_cases(&harness).await;
    let group = cases[0].group.name.as_str();

    // There is no group of this name, so there is no policy to write, read or remove.
    for answer in [
        put_permission(&harness, "no-such-group", ACTION_GROUP, &principals(1), None).await,
        get_policy(&harness, "no-such-group").await,
        remove_permission(&harness, "no-such-group", ACTION_GROUP, FOREIGN_REVISION).await,
    ] {
        assert_eq!(
            answer.status,
            404,
            "case unknown group: expected 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case unknown group: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The `Principals` length bounds (1..50), and one count past each of them.
    for (count, expected) in [
        (MIN_PRINCIPALS - 1, 400),
        (MIN_PRINCIPALS, 200),
        (MAX_PRINCIPALS, 200),
        (MAX_PRINCIPALS + 1, 400),
    ] {
        // Each accepted count writes against whatever revision the store holds, so the
        // bound is what the case is testing, never the revision.
        let revision = harness.stored_policy_revision(group);
        let answer = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &principals(count),
            revision.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status,
            expected,
            "case {count} principals: expected {expected}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // "Wildcards are not are supported in the ARNs", and a blank ARN names nobody —
    // neither is modelled, so the handler refuses both as ValidationException.
    for bad in [
        format!("arn:aws:iam::{}:role/*", ::engine::EMULATOR_ACCOUNT_ID),
        "*".to_string(),
        "   ".to_string(),
    ] {
        let revision = harness.stored_policy_revision(group);
        let answer = put_permission(
            &harness,
            group,
            ACTION_GROUP,
            &[principal(0), bad.clone()],
            revision.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case principal {bad:?}: expected 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case principal {bad:?}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            harness.stored_policy_revision(group),
            revision,
            "case principal {bad:?}: a refused write must change nothing"
        );
    }

    // An action group outside the modelled `ActionGroup` enum, on both writes.
    let revision = harness.stored_policy_revision(group);
    for answer in [
        put_permission(
            &harness,
            group,
            "bogusPermissions",
            &principals(1),
            revision.as_deref(),
        )
        .await,
        remove_permission(
            &harness,
            group,
            "bogusPermissions",
            revision.as_deref().unwrap_or(FOREIGN_REVISION),
        )
        .await,
    ] {
        assert_eq!(
            answer.status,
            400,
            "case unmodelled actionGroup: expected 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case unmodelled actionGroup: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}
