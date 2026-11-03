// SPDX-License-Identifier: CC0-1.0

//! `DeleteDeploymentGroup`: removing ONE deployment group from the application
//! that holds it, leaving everything else that application holds alone.
//!
//! Every case goes through the same front door a client uses — the router, the
//! boundary validator and the awsJson1_1 envelope — by reusing the harness the
//! lifecycle suite owns.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, Emulator};

/// The service role every seeded group is created with; CodeDeploy requires one
/// but nothing here depends on its value.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// Create `application` and then every group of `groups` on it, in that order.
///
/// Each group carries a selector derived from its own name, so a group that
/// survives a delete can be told apart from every other one by reading it back.
async fn seed(emulator: &Emulator, application: &str, groups: &[String]) {
    emulator
        .ok("CreateApplication", json!({"applicationName": application}))
        .await;
    for group in groups {
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": SERVICE_ROLE,
                    "autoScalingGroups": [format!("asg-{}", group.len())],
                    "onPremisesInstanceTagFilters": [
                        {"Key": "group", "Value": group, "Type": "KEY_AND_VALUE"},
                    ],
                }),
            )
            .await;
    }
}

/// The names in a `ListDeploymentGroups` answer, in the order it reported them.
async fn listed_groups(emulator: &Emulator, application: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": application}),
        )
        .await["deploymentGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// One group's `deploymentGroupInfo`, read back through `GetDeploymentGroup`.
async fn read_group(emulator: &Emulator, application: &str, group: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

/// Delete `group` from `application`, requiring the success CodeDeploy answers
/// and the "no hook left behind" output.
async fn delete_group(emulator: &Emulator, application: &str, group: &str, case: &str) {
    let answered = emulator
        .ok(
            "DeleteDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await;
    assert_eq!(
        answered["hooksNotCleanedUp"],
        json!([]),
        "case {case:?}: expected no Auto Scaling hook left behind, got {answered:?}"
    );
}

/// RULE: the delete removes exactly the named deployment group from the named
/// application, so later reads of it answer the modelled "does not exist" and
/// listings no longer carry it, while the application, its other groups and its
/// deployment history are untouched — and the freed name can be created again.
#[tokio::test]
async fn deleting_a_group_removes_only_that_group() {
    let emulator = Emulator::new();
    // Rows differ along how many groups the application holds, WHERE in the
    // creation order the deleted one sits, and the FORM of its name: the
    // punctuation row spans every non-alphanumeric character
    // `DeploymentGroupName`'s modelled pattern allows and the bounds rows sit on
    // each end of its modelled length
    // (`smithy_query(query="shape",shape="DeploymentGroupName")`: length 1..100,
    // pattern ^[A-Za-z0-9+=,.@_-]*$).
    let max_name = "g".repeat(100);
    let cases: Vec<(&str, Vec<String>, String)> = vec![
        ("the only group", owned(&["solo"]), "solo".to_string()),
        (
            "the first of three",
            owned(&["first", "second", "third"]),
            "first".to_string(),
        ),
        (
            "the middle of three",
            owned(&["first", "second", "third"]),
            "second".to_string(),
        ),
        (
            "the last of three",
            owned(&["first", "second", "third"]),
            "third".to_string(),
        ),
        (
            "a name of punctuation",
            owned(&["g+1", "g=2", "g,3", "g.4", "g@5", "g_6", "g-7"]),
            "g,3".to_string(),
        ),
        (
            "a one-character name",
            owned(&["g", "gg"]),
            "g".to_string(),
        ),
        (
            "a name on the modelled maximum length",
            vec![max_name.clone(), "short".to_string()],
            max_name.clone(),
        ),
    ];

    for (index, (case, groups, target)) in cases.iter().enumerate() {
        let application = format!("delete-app-{index}");
        seed(&emulator, &application, groups).await;

        // The deployment the group ran, so the delete can be shown not to erase
        // history.
        let deployment_id = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": &application,
                    "deploymentGroupName": target,
                    "revision": s3_revision("bundles", "app.zip"),
                }),
            )
            .await["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let survivors: Vec<String> = groups
            .iter()
            .filter(|group| *group != target)
            .cloned()
            .collect();
        let mut before: Vec<(String, Value)> = Vec::new();
        for group in &survivors {
            before.push((group.clone(), read_group(&emulator, &application, group).await));
        }
        let deleted_id = read_group(&emulator, &application, target).await["deploymentGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        delete_group(&emulator, &application, target, case).await;

        // Gone for the reader: the modelled refusal, not an empty answer.
        let (status, body) = emulator
            .call(
                "GetDeploymentGroup",
                json!({"applicationName": &application, "deploymentGroupName": target}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentGroupDoesNotExistException"),
            "case {case:?}: expected 400 DeploymentGroupDoesNotExistException for the deleted \
             group, got {status} {body:?}"
        );

        // Gone for the listing, with the surviving groups still in creation
        // order.
        assert_eq!(
            listed_groups(&emulator, &application).await,
            survivors,
            "case {case:?}: expected only {survivors:?} listed after deleting {target:?}"
        );

        // Every surviving group reads back exactly as it did before the delete.
        for (group, expected) in &before {
            let after = read_group(&emulator, &application, group).await;
            assert_eq!(
                &after, expected,
                "case {case:?}: expected {group:?} untouched by deleting {target:?}, got {after:?}"
            );
        }

        // The application itself survives, and so does the deployment the
        // deleted group ran.
        let application_info = emulator
            .ok("GetApplication", json!({"applicationName": &application}))
            .await["application"]
            .clone();
        assert_eq!(
            application_info["applicationName"].as_str(),
            Some(application.as_str()),
            "case {case:?}: expected the application to survive, got {application_info:?}"
        );
        let deployment = emulator
            .ok("GetDeployment", json!({"deploymentId": &deployment_id}))
            .await["deploymentInfo"]
            .clone();
        assert_eq!(
            deployment["deploymentGroupName"].as_str(),
            Some(target.as_str()),
            "case {case:?}: expected the deployment history kept, got {deployment:?}"
        );

        // The name is free again, and re-creating it is a NEW group: a fresh id,
        // appended to the creation order.
        let recreated = emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": &application,
                    "deploymentGroupName": target,
                    "serviceRoleArn": SERVICE_ROLE,
                }),
            )
            .await;
        assert!(
            recreated["deploymentGroupId"].as_str().is_some_and(|id| id != deleted_id),
            "case {case:?}: expected a new deploymentGroupId after re-creating {target:?}, got \
             {recreated:?} against {deleted_id:?}"
        );
        let mut expected_order = survivors.clone();
        expected_order.push(target.clone());
        assert_eq!(
            listed_groups(&emulator, &application).await,
            expected_order,
            "case {case:?}: expected {target:?} appended after re-creation"
        );
    }
}

/// RULE: the delete is IDEMPOTENT — it models no "does not exist" error for
/// either name (`smithy_query(query="operation",operation="DeleteDeploymentGroup")`
/// declares only the four name refusals and `InvalidRoleException`), so a group
/// nothing holds is already in the state the caller asked for and the call
/// succeeds, changing nothing.
#[tokio::test]
async fn deleting_a_group_nothing_holds_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let groups = owned(&["keep-one", "keep-two"]);
    seed(&emulator, "idempotent-app", &groups).await;
    seed(&emulator, "emptied-app", &owned(&["gone"])).await;
    delete_group(&emulator, "emptied-app", "gone", "the first delete").await;
    seed(&emulator, "removed-app", &owned(&["orphan"])).await;
    emulator
        .ok("DeleteApplication", json!({"applicationName": "removed-app"}))
        .await;

    // One row per way a request can name something nothing holds: an unknown
    // group under a known application, a group that was already deleted, a
    // group under an application that was deleted, and a group under an
    // application that never existed.
    let cases: &[(&str, &str, &str)] = &[
        (
            "an unknown group under a known application",
            "idempotent-app",
            "never-created",
        ),
        ("a group deleted a second time", "emptied-app", "gone"),
        (
            "a group under a deleted application",
            "removed-app",
            "orphan",
        ),
        (
            "a group under an application that never existed",
            "no-such-app",
            "no-such-group",
        ),
    ];

    for (case, application, group) in cases {
        delete_group(&emulator, application, group, case).await;
    }

    // Nothing the repeated deletes named moved what the next caller sees.
    assert_eq!(
        listed_groups(&emulator, "idempotent-app").await,
        groups,
        "case the untouched application: expected {groups:?} still listed"
    );
    assert_eq!(
        listed_groups(&emulator, "emptied-app").await,
        Vec::<String>::new(),
        "case an emptied application: expected no groups listed"
    );
    let (status, body) = emulator
        .call("GetApplication", json!({"applicationName": "removed-app"}))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ApplicationDoesNotExistException"),
        "case a deleted application: expected it to stay deleted, got {status} {body:?}"
    );
}

/// RULE: each name the delete carries is refused with CodeDeploy's own code for
/// that member — the `…NameRequiredException` when the request gave none, the
/// `Invalid…NameException` when the value is outside the member's modelled
/// length or pattern.
#[tokio::test]
async fn the_delete_refuses_a_name_it_cannot_accept() {
    let emulator = Emulator::new();
    seed(&emulator, "refusal-app", &owned(&["dg"])).await;
    // Rows span both members and every refusal kind: absent, empty, one past
    // the modelled maximum of 100, and outside the modelled pattern
    // (`smithy_query(query="operation",operation="DeleteDeploymentGroup")`:
    // both names are required, length 1..100, pattern ^[A-Za-z0-9+=,.@_-]*$).
    let too_long = "n".repeat(101);
    let cases: &[(&str, Value, &str)] = &[
        (
            "applicationName absent",
            json!({"deploymentGroupName": "dg"}),
            "ApplicationNameRequiredException",
        ),
        (
            "applicationName empty",
            json!({"applicationName": "", "deploymentGroupName": "dg"}),
            "ApplicationNameRequiredException",
        ),
        (
            "applicationName one past the modelled maximum of 100",
            json!({"applicationName": &too_long, "deploymentGroupName": "dg"}),
            "InvalidApplicationNameException",
        ),
        (
            "applicationName outside the modelled pattern",
            json!({"applicationName": "has spaces", "deploymentGroupName": "dg"}),
            "InvalidApplicationNameException",
        ),
        (
            "deploymentGroupName absent",
            json!({"applicationName": "refusal-app"}),
            "DeploymentGroupNameRequiredException",
        ),
        (
            "deploymentGroupName empty",
            json!({"applicationName": "refusal-app", "deploymentGroupName": ""}),
            "DeploymentGroupNameRequiredException",
        ),
        (
            "deploymentGroupName one past the modelled maximum of 100",
            json!({"applicationName": "refusal-app", "deploymentGroupName": &too_long}),
            "InvalidDeploymentGroupNameException",
        ),
        (
            "deploymentGroupName outside the modelled pattern",
            json!({"applicationName": "refusal-app", "deploymentGroupName": "has spaces"}),
            "InvalidDeploymentGroupNameException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("DeleteDeploymentGroup", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case}: expected 400 {expected}, got {status} {body:?}"
        );
    }

    // A refused request changed nothing.
    assert_eq!(
        listed_groups(&emulator, "refusal-app").await,
        owned(&["dg"]),
        "case after the refusals: expected the seeded group still listed"
    );
}

/// `names` as owned names, for seeding and for comparison against a listing.
fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}
