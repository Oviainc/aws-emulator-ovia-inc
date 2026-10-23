// SPDX-License-Identifier: CC0-1.0

//! `GetPolicy` end to end over HTTP, through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: `GetPolicy` reports the resource-based policy
//! STORED on the named profiling group — one `Allow` statement per stored action group,
//! carrying that action group's principals, the actions the action group grants and the
//! group's own stored ARN, beside the stored revision id — and, because its output
//! models `policy` and `revisionId` as required, refuses with 404
//! `ResourceNotFoundException` whenever no policy is stored (the group has none, or the
//! group does not exist). Being a read, it leaves what a later caller sees untouched.
//!
//! Rows differ along what the rule generalises over: how many principals a grant names,
//! what kind of principal ARN they are, how many action groups the policy carries, and
//! the group they are stored on — every row sits on a row of the shared
//! [`group_cases`] table, so the modelled `ProfilingGroupName` length bounds (1 and 255)
//! and the varying compute platforms come from there. No row is privileged; the
//! recorded cases this suite closes (a group with no policy at all) are the refusal
//! rows.
//!
//! The harness is the one `profiling_group_lifecycle` exports for this test target, so
//! the router wiring is assembled in exactly one place.
//!
//! The policy itself is seeded into the stored state here, so what this suite drives
//! over HTTP is the READ alone; the operations that WRITE a policy, `PutPermission` and
//! `RemovePermission`, are driven over HTTP by [`crate::permission_writes`].

use std::collections::BTreeMap;

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ACTION_GROUP_AGENT_PERMISSIONS, POLICY_VERSION, ResourcePolicy, action_group_actions,
};

use crate::profiling_group_lifecycle::{Answer, GroupCase, Harness, group_cases};

/// The revision a seeded policy starts at, before the seeding rotates it to the case's
/// own revision — so a response echoing the policy's FIRST revision rather than its
/// current one fails every row.
const PLACEHOLDER_REVISION: &str = "00000000-0000-0000-0000-000000000000";

/// `GET /profilingGroups/{name}/policy`, the operation's modelled HTTP binding.
async fn get_policy(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/profilingGroups/{name}/policy"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// `DELETE /profilingGroups/{name}`.
async fn delete_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// Attach `grants` to the stored group as its resource-based policy at `revision_id`,
/// through the same state writers a permission handler uses: rewrite the grants, then
/// rotate the revision ONCE for the change.
fn attach_policy(
    harness: &Harness,
    name: &str,
    grants: &[(String, Vec<String>)],
    revision_id: &str,
) {
    let state = harness
        .engine()
        .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let mut guard = state.lock();
    let group = guard
        .groups
        .get_mut(name)
        .unwrap_or_else(|| panic!("no stored group named {name}"));
    let mut policy = ResourcePolicy::new(PLACEHOLDER_REVISION);
    for (action_group, principals) in grants {
        policy.grant(action_group, principals.clone());
    }
    policy.set_revision(revision_id);
    group.policy = Some(policy);
}

/// Whether the stored group carries a policy right now, as the reader sees it.
fn has_stored_policy(harness: &Harness, name: &str) -> bool {
    let state = harness
        .engine()
        .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let guard = state.lock();
    guard
        .groups
        .get(name)
        .unwrap_or_else(|| panic!("no stored group named {name}"))
        .resource_policy()
        .is_some()
}

/// Whether the stored group holds a policy RECORD at all — the raw field, so the claim
/// that a read wrote nothing does not lean on `resource_policy`'s empty-grants filter.
fn holds_a_policy_record(harness: &Harness, name: &str) -> bool {
    let state = harness
        .engine()
        .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let guard = state.lock();
    guard
        .groups
        .get(name)
        .unwrap_or_else(|| panic!("no stored group named {name}"))
        .policy
        .is_some()
}

/// One stored policy, and the group it is stored on.
#[derive(Debug)]
struct PolicyCase {
    /// A row of the shared profiling-group table — where the group-name and
    /// compute-platform variety comes from.
    group: GroupCase,
    /// `actionGroup` -> the principals it is granted to, in grant order.
    grants: Vec<(String, Vec<String>)>,
    /// The stored revision, in the modelled `RevisionId` form
    /// (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`).
    revision_id: String,
}

/// The table: one policy per row of the shared group table, the rows differing in
/// principal count (1, 2, 3, 1 again on the longest group name, and 50 — the upper bound
/// the model's `Principals` list declares, length 1..50), in principal ARN kind (role,
/// user, account root, assumed-role session, service-linked role path) and in revision
/// id. The account id is the emulator's own, never a literal.
fn policy_cases() -> Vec<PolicyCase> {
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    let principal_rows: Vec<(Vec<String>, &str)> = vec![
        (
            vec![format!("arn:aws:iam::{account}:role/ProfilerAgent")],
            "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
        ),
        (
            vec![
                format!("arn:aws:iam::{account}:role/Testers"),
                format!("arn:aws:iam::{account}:user/analyst"),
            ],
            "ffffffff-0000-1111-2222-333344445555",
        ),
        (
            vec![
                format!("arn:aws:iam::{account}:root"),
                format!("arn:aws:sts::{account}:assumed-role/Deployer/session"),
                format!("arn:aws:iam::{account}:role/service-role/lambda-profiler"),
            ],
            "12345678-90ab-cdef-0123-456789abcdef",
        ),
        (
            vec![format!("arn:aws:iam::{account}:user/single")],
            "abcdef01-2345-6789-abcd-ef0123456789",
        ),
        (
            (0..50)
                .map(|index| format!("arn:aws:iam::{account}:role/bulk-{index}"))
                .collect(),
            "deadbeef-dead-beef-dead-beefdeadbeef",
        ),
    ];

    let groups = group_cases();
    assert_eq!(
        groups.len(),
        principal_rows.len(),
        "every shared group row must carry a principal row, got {} groups and {} rows",
        groups.len(),
        principal_rows.len()
    );

    groups
        .into_iter()
        .zip(principal_rows)
        .map(|(group, (principals, revision_id))| PolicyCase {
            group,
            grants: vec![(ACTION_GROUP_AGENT_PERMISSIONS.to_string(), principals)],
            revision_id: revision_id.to_string(),
        })
        .collect()
}

/// Assert the answer is the document and revision the case stored.
fn assert_matches_case(case: &PolicyCase, label: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?} ({label}): GetPolicy must answer 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    assert_eq!(
        body.get("revisionId").and_then(serde_json::Value::as_str),
        Some(case.revision_id.as_str()),
        "case {case:?} ({label}): wrong revisionId in {body:?}"
    );

    let document_text = body
        .get("policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?} ({label}): no policy string in {body:?}"));
    let document: serde_json::Value = serde_json::from_str(document_text).unwrap_or_else(|error| {
        panic!("case {case:?} ({label}): policy is not JSON ({error}): {document_text}")
    });
    assert_eq!(
        document.get("Version").and_then(serde_json::Value::as_str),
        Some(POLICY_VERSION),
        "case {case:?} ({label}): wrong policy Version in {document}"
    );

    let statements = document
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?} ({label}): no Statement array in {document}"));
    assert_eq!(
        statements.len(),
        case.grants.len(),
        "case {case:?} ({label}): expected {} statement(s), got {statements:?}",
        case.grants.len()
    );

    for (action_group, principals) in &case.grants {
        let statement = statements
            .iter()
            .find(|statement| {
                statement.get("Sid").and_then(serde_json::Value::as_str) == Some(action_group)
            })
            .unwrap_or_else(|| {
                panic!("case {case:?} ({label}): no statement for {action_group} in {document}")
            });
        assert_eq!(
            statement.get("Effect").and_then(serde_json::Value::as_str),
            Some("Allow"),
            "case {case:?} ({label}): wrong Effect in {statement}"
        );
        assert_eq!(
            statement.pointer("/Principal/AWS"),
            Some(&serde_json::Value::from(principals.clone())),
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
            "case {case:?} ({label}): wrong Resource in {statement}"
        );
    }
}

#[tokio::test]
async fn get_policy_renders_the_stored_policy_of_the_named_group() {
    let harness = Harness::new();
    let cases = policy_cases();

    for (index, case) in cases.iter().enumerate() {
        assert_eq!(
            harness
                .create(&case.group, &format!("tok-{index}"))
                .await
                .status,
            201,
            "case {case:?}: the group must be created first"
        );
        attach_policy(
            &harness,
            &case.group.name,
            &case.grants,
            &case.revision_id,
        );
    }

    // Every group is stored at once, so a row that answered with another row's policy
    // (or another row's ARN) is caught here rather than hidden by a fresh store.
    for case in &cases {
        let first = get_policy(&harness, &case.group.name).await;
        assert_matches_case(case, "first read", &first);

        // A read must not change what a later caller sees: same bytes, same revision.
        let second = get_policy(&harness, &case.group.name).await;
        assert_matches_case(case, "second read", &second);
        assert_eq!(
            first.body, second.body,
            "case {case:?}: two reads must answer identically, got {} then {}",
            String::from_utf8_lossy(&first.body),
            String::from_utf8_lossy(&second.body)
        );
    }
}

#[tokio::test]
async fn a_policy_carrying_several_action_groups_renders_one_statement_each() {
    let harness = Harness::new();
    let account = ::engine::EMULATOR_ACCOUNT_ID;
    // A second action group is not a value `PutPermission` accepts today, so it is NOT
    // a modelled enum member: what this row pins is that the renderer emits one
    // statement per STORED action group rather than assuming a single one, and that an
    // action group outside the catalog grants no actions (see `action_group_actions`).
    let case = PolicyCase {
        group: group_cases().swap_remove(0),
        grants: vec![
            (
                ACTION_GROUP_AGENT_PERMISSIONS.to_string(),
                vec![format!("arn:aws:iam::{account}:role/Agent")],
            ),
            (
                "internalAgentPermissions".to_string(),
                vec![format!("arn:aws:iam::{account}:role/Internal")],
            ),
        ],
        revision_id: "99887766-5544-3322-1100-aabbccddeeff".to_string(),
    };

    assert_eq!(
        harness.create(&case.group, "tok-multi").await.status,
        201,
        "case {case:?}: the group must be created first"
    );
    attach_policy(
        &harness,
        &case.group.name,
        &case.grants,
        &case.revision_id,
    );

    let answer = get_policy(&harness, &case.group.name).await;
    assert_matches_case(&case, "multi action group", &answer);
}

#[tokio::test]
async fn no_stored_policy_answers_resource_not_found_and_stores_nothing() {
    let harness = Harness::new();

    // Rows: every group of the shared table, created and never granted anything (the
    // recorded "absent before any permission" case, across both name-length bounds);
    // the first of them then has a policy holding NO grants attached, because no grants
    // is no policy; and a name that was never created at all.
    let groups = group_cases();
    for (index, case) in groups.iter().enumerate() {
        assert_eq!(
            harness.create(case, &format!("tok-{index}")).await.status,
            201,
            "case {case:?}: the group must be created first"
        );
    }
    attach_policy(&harness, &groups[0].name, &[], PLACEHOLDER_REVISION);

    let mut names: Vec<String> = groups.iter().map(|case| case.name.clone()).collect();
    names.push("never-created-group".to_string());

    for name in &names {
        for label in ["first read", "second read"] {
            let answer = get_policy(&harness, name).await;
            assert_eq!(
                answer.status,
                404,
                "case {name} ({label}): a group with no policy must answer 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {name} ({label}): wrong error code in {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    // The refusal is a read too: it must not have attached a policy to anything. For
    // every group that was never granted, the raw policy FIELD must still be empty —
    // evidence of no write that is independent of the empty-grants filter; the seeded
    // empty-grants row keeps its (grant-less) record, and must still read as absent.
    for (index, case) in groups.iter().enumerate() {
        assert!(
            !has_stored_policy(&harness, &case.name),
            "case {case:?}: GetPolicy must not attach a policy, but one is readable"
        );
        if index > 0 {
            assert!(
                !holds_a_policy_record(&harness, &case.name),
                "case {case:?}: GetPolicy must write nothing, but a policy record is stored"
            );
        }
    }
    assert!(
        holds_a_policy_record(&harness, &groups[0].name),
        "case {:?}: the seeded grant-less policy record must survive the reads",
        groups[0]
    );
}

#[tokio::test]
async fn deleting_a_group_takes_its_policy_with_it() {
    let harness = Harness::new();
    let case = &policy_cases().swap_remove(0);
    let name = case.group.name.clone();

    assert_eq!(
        harness.create(&case.group, "tok-delete").await.status,
        201,
        "the group must be created first"
    );
    attach_policy(&harness, &name, &case.grants, &case.revision_id);
    assert_matches_case(case, "before delete", &get_policy(&harness, &name).await);

    assert_eq!(
        delete_group(&harness, &name).await.status,
        204,
        "the group must delete"
    );
    let gone = get_policy(&harness, &name).await;
    assert_eq!(
        gone.status,
        404,
        "a deleted group has no policy to report, got {} {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );

    assert_eq!(
        harness.create(&case.group, "tok-recreate").await.status,
        201,
        "the group must be creatable again"
    );
    let recreated = get_policy(&harness, &name).await;
    assert_eq!(
        recreated.status,
        404,
        "a re-created group must start with no policy, got {} {}",
        recreated.status,
        String::from_utf8_lossy(&recreated.body)
    );
    assert_eq!(
        recreated.error_code(),
        "ResourceNotFoundException",
        "a re-created group's GetPolicy must answer ResourceNotFoundException, got {}",
        String::from_utf8_lossy(&recreated.body)
    );
}

#[test]
fn the_action_catalog_is_the_one_the_model_documents() {
    // Source: the model's `PutPermission` doc for `actionGroup` — "One action group is
    // supported, agentPermissions, which grants permission to perform actions required
    // by the profiling agent, ConfigureAgent and PostAgentProfile permissions."
    // (`smithy_query(query="operation",operation="PutPermission")` for the member.)
    let expected: BTreeMap<&str, Vec<&str>> = BTreeMap::from([
        (
            ACTION_GROUP_AGENT_PERMISSIONS,
            vec![
                "codeguru-profiler:ConfigureAgent",
                "codeguru-profiler:PostAgentProfile",
            ],
        ),
        // Not in the catalog: an action group the service does not define grants
        // nothing, rather than a guessed action list.
        ("internalAgentPermissions", Vec::new()),
        ("", Vec::new()),
    ]);

    for (action_group, actions) in expected {
        assert_eq!(
            action_group_actions(action_group),
            actions,
            "case {action_group:?}: wrong actions, got {:?}",
            action_group_actions(action_group)
        );
    }
}
