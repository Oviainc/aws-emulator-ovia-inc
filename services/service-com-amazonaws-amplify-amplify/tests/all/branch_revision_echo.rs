// SPDX-License-Identifier: CC0-1.0

//! What `ListBranches` and `DeleteBranch` answer with, for a branch that has been
//! updated since it was created.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses.
//!
//! The rules under test:
//!
//! 1. **Current revision.** Every operation that answers with a branch reads the
//!    stored record, so the body carries the members as the LAST write left them —
//!    `ListBranches` and `DeleteBranch` as much as `GetBranch`, and whichever member
//!    the update changed.
//! 2. **One projection.** A `ListBranches` element, a `GetBranch` body and a
//!    `DeleteBranch` echo are the same record: every modelled `Branch` member is
//!    present on each, JSON `null` where the record has no value, nested members
//!    (`backend.stackArn`) included.
//! 3. **The provisioned auto-build setting persists.** Whatever `enableAutoBuild` a
//!    branch was created with is what every later answer about it reports — the
//!    delete echo included — and it does not track the app's own
//!    `enableBranchAutoBuild`.
//! 4. **A read changes nothing.** Listing or fetching a branch leaves what the next
//!    caller sees untouched.
//!
//! Each table spans the class its rule generalises over — which member the update
//! changes and of what type (string, enum, numeric-string, boolean, map, nested
//! structure), the modelled bounds of `ttl`, and every value of the `Stage` enum —
//! rather than the one recorded request (`stage` BETA, `ttl` "10").

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, without_tags, Emulator, Reply};

/// Every member modelled on `com.amazonaws.amplify#Branch`, as the wire names
/// `smithy_query(query="shape",shape="Branch")` lists them.
///
/// AWS answers with the COMPLETE record, so each of these is a key of every branch
/// body — carrying `null` when the branch has no value for it.
const MODELLED_BRANCH_MEMBERS: [&str; 30] = [
    "activeJobId",
    "associatedResources",
    "backend",
    "backendEnvironmentArn",
    "basicAuthCredentials",
    "branchArn",
    "branchName",
    "buildSpec",
    "computeRoleArn",
    "createTime",
    "customDomains",
    "description",
    "destinationBranch",
    "displayName",
    "enableAutoBuild",
    "enableBasicAuth",
    "enableNotification",
    "enablePerformanceMode",
    "enablePullRequestPreview",
    "enableSkewProtection",
    "environmentVariables",
    "framework",
    "pullRequestEnvironmentName",
    "sourceBranch",
    "stage",
    "tags",
    "thumbnailUrl",
    "totalNumberOfJobs",
    "ttl",
    "updateTime",
];

/// Assert that `record` carries every member of `expected` with exactly that value.
fn assert_members(label: &str, operation: &str, record: &Value, expected: &Value) {
    for (member, want) in expected
        .as_object()
        .expect("every expectation table entry is a JSON object")
    {
        assert_eq!(
            record.get(member),
            Some(want),
            "case {label:?}: {operation} must report {member:?} as {want}, got {:?} in \
             {record:?}",
            record.get(member)
        );
    }
}

/// Assert that `record` is a COMPLETE `Branch`: every modelled member present (a
/// member with no value is present and `null`), and nothing else.
fn assert_complete_branch(label: &str, operation: &str, record: &Value) {
    let members = record
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: {operation} must answer a branch object"));
    for member in MODELLED_BRANCH_MEMBERS {
        assert!(
            members.contains_key(member),
            "case {label:?}: {operation} must carry the modelled member {member:?}, got \
             {record:?}"
        );
    }
    for member in members.keys() {
        assert!(
            MODELLED_BRANCH_MEMBERS.contains(&member.as_str()),
            "case {label:?}: {operation} carries {member:?}, which Branch does not model, \
             got {record:?}"
        );
    }
    // A nested record is as complete as the record carrying it, so `backend` is an
    // object with its own member present whenever the branch has a backend at all.
    if let Some(backend) = members.get("backend") {
        if !backend.is_null() {
            assert!(
                backend.get("stackArn").is_some(),
                "case {label:?}: {operation} must carry backend.stackArn (null when unset), \
                 got {backend:?}"
            );
        }
    }
}

/// The element of a `ListBranches` page that names `branch_name`.
fn listed_branch(label: &str, listed: &Reply, branch_name: &str) -> Value {
    listed
        .body
        .get("branches")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: ListBranches answers with a branches list"))
        .iter()
        .find(|element| element.get("branchName").and_then(Value::as_str) == Some(branch_name))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {label:?}: ListBranches must list the branch {branch_name:?}")
        })
}

/// One revision case: the branch to create, the update to apply, and the members
/// every later answer must then carry.
struct RevisionCase {
    case: &'static str,
    create: Value,
    update: Value,
    /// Member name -> the exact JSON value every post-update answer must carry.
    expected: Value,
}

/// The revision table. Each row changes a DIFFERENT kind of member, so the rule is
/// exercised over the class it generalises over rather than over `stage`/`ttl` alone:
///
/// * the enum member and the numeric-string member the recording cites, promoted to
///   values it does not (`PRODUCTION`, `ttl` "86400");
/// * `stage` over every value of the modelled enum (`BETA | DEVELOPMENT |
///   EXPERIMENTAL | PRODUCTION | PULL_REQUEST`, from
///   `smithy_query(query="shape",shape="Stage")`);
/// * `ttl` at both modelled `length` bounds (`length 0..32, pattern ^\d*$`, from
///   `smithy_query(query="operation",operation="UpdateBranch")`): the empty value and
///   32 digits;
/// * free-text members, a boolean member each way, the environment map replaced, and
///   the nested `backend` structure.
fn revision_cases() -> Vec<RevisionCase> {
    vec![
        RevisionCase {
            case: "a development branch promoted with a longer cache lifetime",
            create: json!({ "stage": "DEVELOPMENT", "ttl": "5" }),
            update: json!({ "stage": "PRODUCTION", "ttl": "86400" }),
            expected: json!({ "stage": "PRODUCTION", "ttl": "86400" }),
        },
        RevisionCase {
            case: "stage BETA",
            create: json!({ "stage": "DEVELOPMENT" }),
            update: json!({ "stage": "BETA" }),
            expected: json!({ "stage": "BETA" }),
        },
        RevisionCase {
            case: "stage EXPERIMENTAL",
            create: json!({ "stage": "PRODUCTION" }),
            update: json!({ "stage": "EXPERIMENTAL" }),
            expected: json!({ "stage": "EXPERIMENTAL" }),
        },
        RevisionCase {
            case: "stage PULL_REQUEST with its environment name",
            create: json!({ "stage": "BETA" }),
            update: json!({
                "stage": "PULL_REQUEST",
                "pullRequestEnvironmentName": "pr-42",
                "enablePullRequestPreview": true,
            }),
            expected: json!({
                "stage": "PULL_REQUEST",
                "pullRequestEnvironmentName": "pr-42",
                "enablePullRequestPreview": true,
            }),
        },
        RevisionCase {
            case: "stage DEVELOPMENT",
            create: json!({ "stage": "PRODUCTION" }),
            update: json!({ "stage": "DEVELOPMENT" }),
            expected: json!({ "stage": "DEVELOPMENT" }),
        },
        RevisionCase {
            case: "ttl emptied, at the modelled length minimum",
            create: json!({ "ttl": "5" }),
            update: json!({ "ttl": "" }),
            expected: json!({ "ttl": "" }),
        },
        RevisionCase {
            case: "ttl at the modelled length maximum of 32 digits",
            create: json!({ "ttl": "5" }),
            update: json!({ "ttl": "99999999999999999999999999999999" }),
            expected: json!({ "ttl": "99999999999999999999999999999999" }),
        },
        RevisionCase {
            case: "free-text members rewritten",
            create: json!({ "description": "first", "framework": "React", "displayName": "d1" }),
            update: json!({
                "description": "second",
                "framework": "Next.js - SSR",
                "displayName": "d2",
            }),
            expected: json!({
                "description": "second",
                "framework": "Next.js - SSR",
                "displayName": "d2",
            }),
        },
        RevisionCase {
            case: "every branch flag switched on",
            create: json!({ "enableAutoBuild": false }),
            update: json!({
                "enableAutoBuild": true,
                "enableNotification": true,
                "enableBasicAuth": true,
                "enablePerformanceMode": true,
                "enableSkewProtection": true,
            }),
            expected: json!({
                "enableAutoBuild": true,
                "enableNotification": true,
                "enableBasicAuth": true,
                "enablePerformanceMode": true,
                "enableSkewProtection": true,
            }),
        },
        RevisionCase {
            case: "auto-build switched off",
            create: json!({ "enableAutoBuild": true }),
            update: json!({ "enableAutoBuild": false }),
            expected: json!({ "enableAutoBuild": false }),
        },
        RevisionCase {
            case: "the environment map replaced, not merged",
            create: json!({ "environmentVariables": { "A": "1", "B": "2" } }),
            update: json!({ "environmentVariables": { "C": "3" } }),
            expected: json!({ "environmentVariables": { "C": "3" } }),
        },
        // `Backend$stackArn` is modelled `length 20..2048, pattern
        // ^arn:aws:cloudformation:[a-z0-9-]+:\d{12}:stack/.+/.+$`
        // (`smithy_query(query="shape",shape="Backend")`).
        RevisionCase {
            case: "the nested backend given a CloudFormation stack",
            create: json!({}),
            update: json!({
                "backend": {
                    "stackArn": "arn:aws:cloudformation:us-west-2:000000000000:stack/amplify-gen2/abcd",
                },
                "backendEnvironmentArn": "arn:aws:amplify:us-west-2:000000000000:apps/d1/backendenvironments/dev",
                "buildSpec": "version: 1\n",
                "computeRoleArn": "arn:aws:iam::000000000000:role/amplify-ssr",
            }),
            expected: json!({
                "backend": {
                    "stackArn": "arn:aws:cloudformation:us-west-2:000000000000:stack/amplify-gen2/abcd",
                },
                "backendEnvironmentArn": "arn:aws:amplify:us-west-2:000000000000:apps/d1/backendenvironments/dev",
                "buildSpec": "version: 1\n",
                "computeRoleArn": "arn:aws:iam::000000000000:role/amplify-ssr",
            }),
        },
    ]
}

#[tokio::test]
async fn list_and_delete_answer_the_branch_at_its_current_revision() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("revision-host").await;

    for (index, case) in revision_cases().into_iter().enumerate() {
        let label = case.case;
        let branch_name = format!("revision-{index}");

        let mut create = case.create.clone();
        create["branchName"] = json!(branch_name);
        let created = emulator.create_branch(&app_id, &create).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );

        // A sibling branch exists throughout the row: the update must reach the
        // addressed branch only.
        let sibling = format!("sibling-{index}");
        let sibling_created = emulator
            .create_branch(&app_id, &json!({ "branchName": sibling, "stage": "BETA" }))
            .await;
        assert_eq!(
            sibling_created.status, 200,
            "case {label:?}: expected HTTP 200 for the sibling CreateBranch, got \
             {sibling_created:?}"
        );
        let sibling_before = emulator.get_branch(&app_id, &sibling).await.member("branch");

        let updated = emulator
            .update_branch(&app_id, &branch_name, &case.update)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateBranch, got {updated:?}"
        );
        let updated_record = updated.member("branch");
        assert_members(label, "UpdateBranch", &updated_record, &case.expected);
        assert_complete_branch(label, "UpdateBranch", &updated_record);

        // The canonical read, the list element and the delete echo all answer the
        // record the update left behind.
        let fetched = emulator
            .get_branch(&app_id, &branch_name)
            .await
            .member("branch");
        assert_members(label, "GetBranch", &fetched, &case.expected);
        assert_complete_branch(label, "GetBranch", &fetched);

        let listed = emulator.list_branches(&app_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected HTTP 200 from ListBranches, got {listed:?}"
        );
        let element = listed_branch(label, &listed, &branch_name);
        assert_members(label, "ListBranches", &element, &case.expected);
        assert_complete_branch(label, "ListBranches", &element);
        assert_eq!(
            element, fetched,
            "case {label:?}: a listed branch and a fetched branch are the same record, got \
             {element:?}"
        );

        // A read changed nothing: the second list and a re-read agree with the first.
        let listed_again = emulator.list_branches(&app_id, "").await;
        assert_eq!(
            listed_branch(label, &listed_again, &branch_name),
            element,
            "case {label:?}: listing a branch must not change what a later caller sees"
        );
        assert_eq!(
            emulator
                .get_branch(&app_id, &branch_name)
                .await
                .member("branch"),
            fetched,
            "case {label:?}: reading a branch must not change what a later caller sees"
        );
        assert_eq!(
            without_tags(&emulator.get_branch(&app_id, &sibling).await.member("branch")),
            without_tags(&sibling_before),
            "case {label:?}: updating one branch must not touch another"
        );

        let deleted = emulator.delete_branch(&app_id, &branch_name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteBranch, got {deleted:?}"
        );
        let delete_echo = deleted.member("branch");
        assert_members(label, "DeleteBranch", &delete_echo, &case.expected);
        assert_complete_branch(label, "DeleteBranch", &delete_echo);
        // The last observation a client gets of the resource is the record as it
        // stood, which is what the read answered a moment earlier.
        assert_eq!(
            without_tags(&delete_echo),
            without_tags(&fetched),
            "case {label:?}: DeleteBranch must echo the branch at its current revision, got \
             {delete_echo:?}"
        );

        assert_eq!(
            emulator.delete_branch(&app_id, &sibling).await.status,
            200,
            "case {label:?}: expected HTTP 200 from the sibling cleanup DeleteBranch"
        );
    }
}

#[tokio::test]
async fn an_update_moves_only_the_members_it_names() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("partial-update-host").await;

    let created = emulator
        .create_branch(
            &app_id,
            &json!({
                "branchName": "partial",
                "stage": "DEVELOPMENT",
                "ttl": "5",
                "description": "kept",
                "framework": "React",
                "environmentVariables": { "KEEP": "yes" },
                "enableNotification": true,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let before = emulator
        .get_branch(&app_id, "partial")
        .await
        .member("branch");

    // An update naming ONE member leaves every other member of the record alone —
    // `updateTime` excepted, which is when the configuration last changed.
    let updated = emulator
        .update_branch(&app_id, "partial", &json!({ "stage": "BETA" }))
        .await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200 from UpdateBranch, got {updated:?}"
    );
    let after = emulator
        .get_branch(&app_id, "partial")
        .await
        .member("branch");

    for member in MODELLED_BRANCH_MEMBERS {
        if member == "stage" || member == "updateTime" {
            continue;
        }
        assert_eq!(
            after.get(member),
            before.get(member),
            "case \"one-member update\": {member:?} was not named by the update and must not \
             change, got {:?} (was {:?})",
            after.get(member),
            before.get(member)
        );
    }
    assert_eq!(
        string_member(&after, "stage"),
        "BETA",
        "case \"one-member update\": the named member must change, got {:?}",
        after.get("stage")
    );
    assert_eq!(
        after.get("createTime"),
        before.get("createTime"),
        "case \"one-member update\": createTime is when the branch was created, got {:?}",
        after.get("createTime")
    );

    // An update addressed to a branch that does not exist writes nothing and answers
    // the modelled 404 — the same refusal every other branch-scoped operation gives.
    let absent = emulator
        .update_branch(&app_id, "no-such-branch", &json!({ "stage": "BETA" }))
        .await;
    assert_eq!(
        absent.status, 404,
        "case \"update of an absent branch\": expected HTTP 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code.as_deref(),
        Some("NotFoundException"),
        "case \"update of an absent branch\": expected NotFoundException, got {:?}",
        absent.error_code
    );
    let absent_app = emulator
        .update_branch("dnosuchapp1", "partial", &json!({ "stage": "BETA" }))
        .await;
    assert_eq!(
        absent_app.status, 404,
        "case \"update under an absent app\": expected HTTP 404, got {absent_app:?}"
    );
}

/// One auto-build case: what the create request says about `enableAutoBuild`, and the
/// value every answer about that branch must then carry.
struct AutoBuildCase {
    case: &'static str,
    request: Value,
    expected: bool,
}

/// The auto-build table.
///
/// The rule this suite asserts is PERSISTENCE: whatever `enableAutoBuild` the branch
/// was provisioned with is what every later answer about it reports, the delete echo
/// included — so the rows cover the member set true, set false, and absent. The value
/// an ABSENT member takes is the handler's documented default
/// (`DEFAULT_ENABLE_AUTO_BUILD` in `src/ops/create_branch.rs`, which cites the
/// recorded `CreateBranch` whose request omits the member), not a rule this suite
/// establishes; the first row pins only that whatever that default is, it survives to
/// the delete echo unchanged.
fn auto_build_cases() -> Vec<AutoBuildCase> {
    vec![
        AutoBuildCase {
            case: "enableAutoBuild absent",
            request: json!({}),
            expected: true,
        },
        AutoBuildCase {
            case: "enableAutoBuild true",
            request: json!({ "enableAutoBuild": true }),
            expected: true,
        },
        AutoBuildCase {
            case: "enableAutoBuild false",
            request: json!({ "enableAutoBuild": false }),
            expected: false,
        },
    ]
}

#[tokio::test]
async fn the_provisioned_auto_build_setting_survives_to_the_delete_echo() {
    let emulator = Emulator::new();

    // The app's own `enableBranchAutoBuild` is varied too, so the branch's value is
    // shown to come from the branch's provisioning and not from the app's flag.
    for app_flag in [None, Some(false), Some(true)] {
        let mut app_request = json!({ "name": format!("auto-build-host-{app_flag:?}") });
        if let Some(flag) = app_flag {
            app_request["enableBranchAutoBuild"] = json!(flag);
        }
        let app = emulator.create_app(&app_request).await;
        assert_eq!(
            app.status, 200,
            "expected HTTP 200 from CreateApp, got {app:?}"
        );
        let app_id = string_member(&app.member("app"), "appId");

        for (index, case) in auto_build_cases().into_iter().enumerate() {
            let label = format!("{} (app flag {app_flag:?})", case.case);
            let branch_name = format!("auto-build-{index}");
            let mut request = case.request.clone();
            request["branchName"] = json!(branch_name);

            let created = emulator.create_branch(&app_id, &request).await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
            );
            let expected = json!({ "enableAutoBuild": case.expected });
            assert_members(&label, "CreateBranch", &created.member("branch"), &expected);

            let fetched = emulator
                .get_branch(&app_id, &branch_name)
                .await
                .member("branch");
            assert_members(&label, "GetBranch", &fetched, &expected);

            let listed = emulator.list_branches(&app_id, "").await;
            let element = listed_branch(&label, &listed, &branch_name);
            assert_members(&label, "ListBranches", &element, &expected);

            let deleted = emulator.delete_branch(&app_id, &branch_name).await;
            assert_eq!(
                deleted.status, 200,
                "case {label:?}: expected HTTP 200 from DeleteBranch, got {deleted:?}"
            );
            assert_members(&label, "DeleteBranch", &deleted.member("branch"), &expected);
        }
    }
}
