// SPDX-License-Identifier: CC0-1.0

//! What `DeleteFleet` removes, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.DeleteFleet` on awsJson1_1.
//!
//! The rule under test: an ARN addresses exactly the compute fleet whose STORED ARN it
//! is, the call starts the deletion of that one record and nothing else, and because the
//! operation models no not-found error a well-formed fleet ARN this account does not
//! hold is a success that changes nothing — while a value that is not a compute fleet ARN
//! of this service is refused with the operation's modelled `InvalidInputException`
//! (HTTP 400). A fleet's capacity is torn down over a span, so a deleted fleet is still
//! reported in the model's `PENDING_DELETION` status until the clock makes its removal
//! from the store due. The builds that ran on it survive it, which is what AWS documents
//! the deletion as leaving behind.
//!
//! Every deletion is read back through `BatchGetFleets` and `ListFleets`, so what is
//! asserted is the store rather than the deleting call's own answer.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test crate drives the service through, and the
// `CreateFleet` helpers the write-end fleet suite already owns, so a fleet under test
// here is seeded exactly as a client would seed it.
use super::fleet_lifecycle::{batch_get_fleets, create_fleet, PAST_THE_FLEET_SETTLE_SPAN};
use super::project_build_lifecycle::{
    call, create_project, emulator, error_type, expected_arn, start_build, REGION,
};

/// Long enough for a deleted fleet's clock-owed removal from the store to have fallen
/// due (the service owes it [`state::FLEET_DELETE_MILLIS`] after `DeleteFleet`).
const PAST_THE_FLEET_DELETE_SPAN: Duration =
    Duration::from_millis(state::FLEET_DELETE_MILLIS as u64 + 350);

/// Delete the compute fleet `arn` names, handing back the HTTP status and body.
async fn delete_fleet(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "DeleteFleet", json!({ "arn": arn })).await
}

/// The fleet `BatchGetFleets` reports for `identifier`, or `Value::Null` when the store
/// holds none.
async fn reported_fleet(router: &MultiServiceRouter, identifier: &str) -> Value {
    batch_get_fleets(router, &[identifier]).await["fleets"][0].clone()
}

/// The ARNs `ListFleets` reports as HELD — the registry read, so a deletion is asserted
/// against the whole store and not only against the identifiers asked about.
async fn listed_fleets(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListFleets", json!({})).await;
    assert_eq!(
        status, 200,
        "ListFleets: expected 200, got {status} with {body}"
    );
    body["fleets"]
        .as_array()
        .unwrap_or_else(|| panic!("ListFleets: no fleets in {body}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The ARN of the fleet `CreateFleet` minted for `name`.
async fn seeded_fleet(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let fleet = create_fleet(router, name, extra).await;
    fleet["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFleet {name:?}: no arn in {fleet}"))
        .to_string()
}

/// One row of the deletion table.
#[derive(Debug)]
struct DeleteCase {
    /// What the row is about, named in every assertion it drives.
    case: &'static str,
    /// The fleet created and then deleted.
    name: &'static str,
    /// `CreateFleet` members merged onto the minimal valid request.
    extra: fn() -> Value,
    /// Whether the fleet is left until the clock has made its move to `ACTIVE` due
    /// before the delete, so the table covers both a fleet still provisioning its
    /// capacity and one that finished.
    settled: bool,
}

/// RULE: `DeleteFleet` starts the deletion of the one compute fleet its ARN names,
/// whatever that fleet holds and whatever status it is in, and leaves every other fleet
/// where it was: the fleet it named reports the model's `PENDING_DELETION` status until
/// the clock makes its removal from the store due, and is gone from the registry after. A
/// repeated delete while the first is still in flight is a success that changes nothing,
/// because the operation models no not-found error.
///
/// The table spans the classes the rule generalises over: the `ComputeType` values that
/// name their machine and the two the model documents as describing it by attributes
/// instead (`smithy_query(shape="ComputeType")` / `Fleet$computeConfiguration`), three
/// `EnvironmentType` values (`smithy_query(shape="EnvironmentType")`), both
/// `FleetOverflowBehavior` values, a capacity at the floor and well inside it, tag lists
/// of 0, 1 and the 50 entries `TagList`'s `@length(min: 0, max: 50)` allows, a scaling
/// and VPC configuration present and absent, and a fleet deleted while still `CREATING`
/// as well as one deleted after it reached `ACTIVE`.
#[tokio::test]
async fn delete_fleet_deletes_the_fleet_its_arn_names_and_nothing_else() {
    let cases = [
        DeleteCase {
            case: "a minimal fleet deleted while still CREATING",
            name: "del-minimal",
            extra: || json!({}),
            settled: false,
        },
        DeleteCase {
            case: "a scaling, tagged, VPC-connected fleet deleted once ACTIVE",
            name: "del-scaling",
            extra: || {
                json!({
                    "baseCapacity": 4,
                    "environmentType": "ARM_CONTAINER",
                    "computeType": "BUILD_GENERAL1_LARGE",
                    "overflowBehavior": "QUEUE",
                    "scalingConfiguration": {
                        "scalingType": "TARGET_TRACKING_SCALING",
                        "maxCapacity": 8,
                        "targetTrackingScalingConfigs": [
                            {"metricType": "FLEET_UTILIZATION_RATE", "targetValue": 0.6},
                        ],
                    },
                    "vpcConfig": {
                        "vpcId": "vpc-0123456789abcdef0",
                        "subnets": ["subnet-0123456789abcdef0"],
                        "securityGroupIds": ["sg-0123456789abcdef0"],
                    },
                    "tags": [{"key": "team", "value": "builders"}],
                })
            },
            settled: true,
        },
        DeleteCase {
            case: "an attribute-based fleet, whose machine is described by attributes",
            name: "del-attribute-based",
            extra: || {
                json!({
                    "baseCapacity": 1,
                    "environmentType": "LINUX_EC2",
                    "computeType": "ATTRIBUTE_BASED_COMPUTE",
                    // `MachineType`: GENERAL | NVME (smithy_query(shape="MachineType")).
                    "computeConfiguration": {
                        "vCpu": 4,
                        "memory": 8,
                        "disk": 64,
                        "machineType": "GENERAL",
                    },
                    "overflowBehavior": "ON_DEMAND",
                    "tags": [],
                })
            },
            settled: false,
        },
        DeleteCase {
            case: "a custom-instance-type fleet carrying 50 tags",
            name: "del-custom-instance",
            extra: || {
                json!({
                    "baseCapacity": 2,
                    "environmentType": "WINDOWS_SERVER_2022_CONTAINER",
                    "computeType": "CUSTOM_INSTANCE_TYPE",
                    "computeConfiguration": {"instanceType": "m6i.large"},
                    "imageId": "ami-0123456789abcdef0",
                    "fleetServiceRole": "arn:aws:iam::000000000000:role/service-role/fleet",
                    "tags": (0..50)
                        .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
                        .collect::<Vec<Value>>(),
                })
            },
            settled: true,
        },
    ];

    for case in &cases {
        let router = emulator();
        let target = seeded_fleet(&router, case.name, (case.extra)()).await;
        // A second fleet, untouched by the deletion, so what is asserted is the ONE
        // record the ARN names rather than the store being emptied.
        let sibling = seeded_fleet(&router, "del-sibling", json!({})).await;
        let mut listed = listed_fleets(&router).await;
        listed.sort();
        let mut both = vec![target.clone(), sibling.clone()];
        both.sort();
        assert_eq!(
            listed, both,
            "case {:?}: both fleets must be held before the delete",
            case.case
        );

        if case.settled {
            tokio::time::sleep(PAST_THE_FLEET_SETTLE_SPAN).await;
            let before = reported_fleet(&router, &target).await;
            assert_eq!(
                before["status"]["statusCode"], "ACTIVE",
                "case {:?}: the fleet must have reached ACTIVE before the delete, got {before}",
                case.case
            );
        }

        let (status, body) = delete_fleet(&router, &target).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteFleet, got {status} with {body}",
            case.case
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {:?}: DeleteFleet models no output member, got {body}",
            case.case
        );

        // The capacity is torn down over a span, so the fleet is still reported until
        // the clock makes its removal due — in the status the model documents for a
        // fleet whose deletion a caller has asked for.
        let pending = reported_fleet(&router, &target).await;
        assert_eq!(
            pending["status"]["statusCode"], "PENDING_DELETION",
            "case {:?}: a fleet read straight after DeleteFleet is PENDING_DELETION, \
             got {pending}",
            case.case
        );
        assert!(
            pending["status"]["context"].is_null(),
            "case {:?}: a fleet pending deletion carries no failure context, got {pending}",
            case.case
        );
        let sibling_record = reported_fleet(&router, &sibling).await;
        assert_ne!(
            sibling_record["status"]["statusCode"], "PENDING_DELETION",
            "case {:?}: the sibling must not be deleted, got {sibling_record}",
            case.case
        );

        // Deleting it again while the first deletion is in flight is a success and
        // changes nothing: the operation models no not-found error.
        let (status, body) = delete_fleet(&router, &target).await;
        assert_eq!(
            status, 200,
            "case {:?}: a repeated delete must answer 200, got {status} with {body}",
            case.case
        );
        let repeated = reported_fleet(&router, &target).await;
        assert_eq!(
            repeated, pending,
            "case {:?}: a repeated delete must report the same record, got {repeated}",
            case.case
        );

        // Once the removal falls due the fleet is gone from the store, and only the
        // sibling is left.
        tokio::time::sleep(PAST_THE_FLEET_DELETE_SPAN).await;
        let listed = listed_fleets(&router).await;
        assert_eq!(
            listed,
            vec![sibling.clone()],
            "case {:?}: only the sibling may survive the delete, got {listed:?}",
            case.case
        );
        let gone = reported_fleet(&router, &target).await;
        assert!(
            gone.is_null(),
            "case {:?}: a removed fleet must not be reported, got {gone}",
            case.case
        );

        // The name is free again, and the fleet created under it is a NEW record: it
        // carries its own id, so the deleted fleet's ARN still addresses nothing.
        let recreated = seeded_fleet(&router, case.name, json!({})).await;
        assert_ne!(
            recreated, target,
            "case {:?}: a recreated fleet must not reuse the deleted fleet's id",
            case.case
        );
        let gone = reported_fleet(&router, &target).await;
        assert!(
            gone.is_null(),
            "case {:?}: the deleted fleet's ARN must address nothing, got {gone}",
            case.case
        );
        let listed = listed_fleets(&router).await;
        assert!(
            listed.contains(&recreated) && !listed.contains(&target),
            "case {:?}: the registry holds the recreated fleet only, got {listed:?}",
            case.case
        );
    }
}

/// RULE: a well-formed compute fleet ARN of a scope this account does not hold addresses
/// no local record, so the call succeeds and the store is untouched — an ARN resolves
/// only when it is EXACTLY a stored record's ARN.
///
/// The table spans the fields such an ARN differs in: its partition, region, account,
/// the fleet name (including its case), the fleet id, a fleet id left off altogether and
/// a name no fleet of this account was created under.
#[tokio::test]
async fn a_fleet_arn_this_account_does_not_hold_deletes_nothing() {
    let router = emulator();
    let held = seeded_fleet(&router, "held-fleet", json!({})).await;
    let id = held
        .rsplit_once(":fleet/")
        .expect("the minted ARN carries a fleet/<id> resource")
        .1
        .to_string();

    let elsewhere = [
        // Another partition.
        held.replacen("arn:aws:", "arn:aws-us-gov:", 1),
        // Another region.
        state::fleet_arn("eu-central-1", &id),
        // Another account.
        format!("arn:aws:codebuild:{REGION}:111122223333:fleet/{id}"),
        // The name in another case — a store keyed by name is case-sensitive.
        held.replace("fleet/held-fleet:", "fleet/HELD-FLEET:"),
        // Another fleet id under the held name.
        format!(
            "{}:00000000-0000-8000-8000-000000000000",
            expected_arn("fleet/held-fleet")
        ),
        // A fleet ARN carrying no fleet id at all.
        expected_arn("fleet/held-fleet"),
        // A name no fleet of this account was ever created under.
        expected_arn("fleet/never-created:00000000-0000-8000-8000-000000000000"),
    ];

    for case in &elsewhere {
        let (status, body) = delete_fleet(&router, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fleet ARN naming no held record must answer 200, \
             got {status} with {body}"
        );
        let still_held = reported_fleet(&router, &held).await;
        assert_eq!(
            still_held["arn"],
            json!(held.clone()),
            "case {case:?}: the held fleet must survive, got {still_held}"
        );
        assert_ne!(
            still_held["status"]["statusCode"], "PENDING_DELETION",
            "case {case:?}: the held fleet must not be deleted, got {still_held}"
        );
    }
}

/// RULE: `arn` is typed `NonEmptyString`, so a value that is not a compute fleet ARN of
/// this service reaches the handler and is refused with the operation's only modelled
/// error, HTTP 400 `InvalidInputException`, persisting nothing.
///
/// The table spans the ways a value fails to be one: not an ARN at all, a bare resource
/// segment, too few ARN fields, an empty partition, another service's ARN, another
/// resource type of this service, and a fleet segment carrying no id.
#[tokio::test]
async fn a_value_that_is_not_a_fleet_arn_is_refused() {
    let router = emulator();
    let held = seeded_fleet(&router, "held-fleet", json!({})).await;
    let id = held
        .rsplit_once(":fleet/")
        .expect("the minted ARN carries a fleet/<id> resource")
        .1
        .to_string();
    let account = engine::EMULATOR_ACCOUNT_ID;

    let cases = [
        // A bare fleet name, which is not an ARN — `DeleteFleet$arn` takes "the ARN of
        // the compute fleet", unlike `BatchGetFleets$names`, which takes either form.
        "held-fleet".to_string(),
        // The fleet's own id, which is not an ARN either.
        id.clone(),
        // The resource segment alone.
        format!("fleet/{id}"),
        // Too few ARN fields.
        format!("arn:aws:codebuild:{REGION}:{account}"),
        // An empty partition.
        format!("arn::codebuild:{REGION}:{account}:fleet/{id}"),
        // Another service's ARN.
        format!("arn:aws:codepipeline:{REGION}:{account}:fleet/{id}"),
        // Another resource type of this service.
        format!("arn:aws:codebuild:{REGION}:{account}:project/held-fleet"),
        // A fleet ARN whose resource segment carries no id.
        format!("arn:aws:codebuild:{REGION}:{account}:fleet/"),
        format!("arn:aws:codebuild:{REGION}:{account}:fleet"),
    ];

    for case in &cases {
        let (status, body) = delete_fleet(&router, case).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(state::INVALID_FLEET_ARN_MESSAGE),
            "case {case:?}: every malformed value is refused with one fixed sentence, got {body}"
        );
        let still_held = reported_fleet(&router, &held).await;
        assert_eq!(
            still_held["arn"],
            json!(held.clone()),
            "case {case:?}: a refused delete must persist nothing, got {still_held}"
        );
        assert_ne!(
            still_held["status"]["statusCode"], "PENDING_DELETION",
            "case {case:?}: a refused delete must not start a deletion, got {still_held}"
        );
    }
}

/// RULE: AWS documents the deletion as leaving the fleet's builds behind ("when you
/// delete a compute fleet, its builds are not deleted"), so the build records and the
/// project configured to run on the fleet survive it; only the fleet record goes.
#[tokio::test]
async fn deleting_a_fleet_leaves_its_builds_and_the_project_that_used_it() {
    let router = emulator();
    let fleet = seeded_fleet(&router, "build-fleet", json!({})).await;

    let project = create_project(
        &router,
        "fleet-project",
        json!({
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
                "fleet": {"fleetArn": fleet.clone()},
            },
        }),
    )
    .await;
    assert_eq!(
        project["environment"]["fleet"]["fleetArn"],
        json!(fleet.clone()),
        "the project must report the fleet it was configured with, got {project}"
    );

    let build = start_build(&router, json!({"projectName": "fleet-project"})).await;
    let build_id = build["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuild: no id in {build}"))
        .to_string();

    let (status, body) = delete_fleet(&router, &fleet).await;
    assert_eq!(
        status, 200,
        "DeleteFleet {fleet:?}: expected 200, got {status} with {body}"
    );

    // Once the removal falls due the fleet is gone from the store; what the deletion
    // left behind is what AWS documents it as leaving.
    tokio::time::sleep(PAST_THE_FLEET_DELETE_SPAN).await;
    let gone = reported_fleet(&router, &fleet).await;
    assert!(
        gone.is_null(),
        "the deleted fleet must not be reported any more, got {gone}"
    );

    let (status, body) = call(&router, "BatchGetBuilds", json!({"ids": [build_id.clone()]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds {build_id:?}: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["builds"][0]["id"],
        json!(build_id.clone()),
        "the build that ran on the deleted fleet must survive it, got {body}"
    );

    let (status, body) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["fleet-project"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetProjects: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["projects"][0]["environment"]["fleet"]["fleetArn"],
        json!(fleet),
        "the project keeps the fleet setting it was configured with, got {body}"
    );
}
