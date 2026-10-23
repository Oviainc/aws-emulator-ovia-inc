// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentInstances` over the wire.
//!
//! RULE: the call answers the instance ids of the deployment targets the
//! deployment's own record holds — the snapshot its deployment group resolved
//! when it was created — in resolution order, narrowed by the
//! `instanceStatusFilter` and `instanceTypeFilter` the request names, one page of
//! `state::PAGE_SIZE` at a time; a deployment on a compute platform that has no
//! instances is refused with `InvalidComputePlatformException`, one that has not
//! started with `DeploymentNotStartedException`, a filter on the environment an
//! instance belongs to is refused on an in-place deployment with
//! `InvalidDeploymentInstanceTypeException`, and a filter value outside the
//! model's enum with the error the model gives this operation for it.
//!
//! The harness — the router, the `X-Amz-Target` request, the error-code helper
//! and the terminal-status poll — is the one `codedeploy_lifecycle` drives the
//! rest of this service with, and the fleet helpers are the ones the
//! `list_deployment_targets` suite drives the same records through: this
//! operation is the deprecated, instance-shaped view of them, so the cases here
//! are about what that view adds, not a second set of helpers.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, terminal_deployment, Emulator};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// A deployment group selector every registered instance of a case matches, so a
/// row's fleet is exactly what its deployment deploys to.
fn fleet_selector() -> Value {
    json!({
        "onPremisesInstanceTagFilters": [
            {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
        ]
    })
}

/// The same selector, deployed blue/green — the only deployment style whose
/// instances carry the environment label an `instanceTypeFilter` selects on.
fn blue_green_selector() -> Value {
    let mut extras = fleet_selector();
    extras["deploymentStyle"] =
        json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"});
    extras
}

/// The `instancesList` of a successful `ListDeploymentInstances` answer.
fn listed(answer: &Value) -> Vec<String> {
    answer["instancesList"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// List the instances of `deployment_id`, with the `request` members `extras`
/// adds on top of the deployment id.
async fn list_with(emulator: &Emulator, deployment_id: &str, extras: &Value) -> Vec<String> {
    let mut request = json!({"deploymentId": deployment_id});
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    listed(&emulator.ok("ListDeploymentInstances", request).await)
}

/// The `targetIds` the newer `ListDeploymentTargets` answers for the same
/// deployment, which is the record this operation is the older view of.
async fn listed_targets(emulator: &Emulator, deployment_id: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeploymentTargets",
            json!({"deploymentId": deployment_id}),
        )
        .await["targetIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Which instances a deployment holds
// ---------------------------------------------------------------------------

/// RULE: the listed instance ids are the targets the deployment's group
/// resolved, in resolution order — the same records the newer
/// `ListDeploymentTargets` answers for that deployment.
#[tokio::test]
async fn the_listed_instances_are_the_ones_the_deployment_group_resolved() {
    // Rows differ along the compute platform this operation serves, along the
    // SELECTOR the group resolves its instances with, along the deployment
    // style, and along how many of the registered instances the selector takes
    // in. The fleet is `(instance name, tags)`, registered in order; a row's
    // expectation is the ids in resolution order.
    struct Case<'a> {
        case: &'a str,
        platform: &'a str,
        extras: Value,
        fleet: &'a [(&'a str, &'a [(&'a str, &'a str)])],
        expected: &'a [&'a str],
    }
    let fleet: &[(&str, &[(&str, &str)])] = &[
        ("host-a", &[("Fleet", "deployable"), ("Rack", "one")]),
        ("host-b", &[("Fleet", "deployable"), ("Rack", "two")]),
        ("host-c", &[("Fleet", "retired")]),
    ];
    let cases: &[Case] = &[
        Case {
            case: "an on-premises tag filter two of the three instances match",
            platform: "Server",
            extras: fleet_selector(),
            fleet,
            expected: &["host-a", "host-b"],
        },
        Case {
            case: "an on-premises tag filter one instance matches",
            platform: "Server",
            extras: json!({
                "onPremisesInstanceTagFilters": [
                    {"Key": "Rack", "Value": "two", "Type": "KEY_AND_VALUE"},
                ]
            }),
            fleet,
            expected: &["host-b"],
        },
        Case {
            case: "an on-premises tag filter on the key alone, matching every instance",
            platform: "Server",
            extras: json!({
                "onPremisesInstanceTagFilters": [{"Key": "Fleet", "Type": "KEY_ONLY"}]
            }),
            fleet,
            expected: &["host-a", "host-b", "host-c"],
        },
        Case {
            case: "the same fleet deployed blue/green",
            platform: "Server",
            extras: blue_green_selector(),
            fleet,
            expected: &["host-a", "host-b"],
        },
        Case {
            case: "an on-premises selector matching nothing registered",
            platform: "Server",
            extras: json!({
                "onPremisesInstanceTagFilters": [
                    {"Key": "Fleet", "Value": "nowhere", "Type": "KEY_AND_VALUE"},
                ]
            }),
            fleet,
            expected: &[],
        },
        Case {
            case: "an ec2 tag filter, which names inventory this service keeps none of",
            platform: "Server",
            extras: json!({
                "ec2TagFilters": [{"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"}]
            }),
            fleet,
            expected: &[],
        },
        Case {
            case: "no selector at all",
            platform: "Server",
            extras: json!({}),
            fleet,
            expected: &[],
        },
        // A Lambda deployment HAS one target — the function its revision's
        // AppSpec names — but that name lives inside the revision, which
        // CodeDeploy's state does not hold, so there is no id to report for it.
        Case {
            case: "lambda, whose target is named inside the revision",
            platform: "Lambda",
            extras: json!({}),
            fleet: &[],
            expected: &[],
        },
    ];

    for case in cases {
        // One emulator per row: the on-premises registry is account-wide, so a
        // row's expectation is only exact over a fleet of its own.
        let emulator = Emulator::new();
        for (instance, tags) in case.fleet {
            register(&emulator, instance, tags).await;
        }
        let application = "fleet-app";
        group_with(&emulator, application, case.platform, &case.extras).await;
        let deployment_id = deploy(&emulator, application).await;
        terminal_deployment(&emulator, &deployment_id, case.case).await;

        let got = list_with(&emulator, &deployment_id, &json!({})).await;
        assert_eq!(
            got, case.expected,
            "case {:?}: expected the instances {:?}, got {got:?}",
            case.case, case.expected
        );
        // The deprecated view and the newer one answer the SAME records.
        let targets = listed_targets(&emulator, &deployment_id).await;
        assert_eq!(
            got, targets,
            "case {:?}: expected the instances to be the deployment's targets {targets:?}, got \
             {got:?}",
            case.case
        );
    }
}

/// RULE: the instances a deployment lists are the ones its group resolved WHEN
/// it was created — deregistering an instance or retargeting the group
/// afterwards does not change what a past deployment deployed to.
#[tokio::test]
async fn the_listing_is_the_fleet_the_deployment_resolved_when_it_was_created() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    let application = "snapshot-app";
    group_with(&emulator, application, "Server", &fleet_selector()).await;

    let first = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &first, "before the fleet changed").await;
    let before = list_with(&emulator, &first, &json!({})).await;
    assert_eq!(
        before,
        vec!["host-a".to_string(), "host-b".to_string()],
        "case {:?}: expected both registered instances, got {before:?}",
        "before the fleet changed"
    );

    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "host-a"}),
        )
        .await;
    register(&emulator, "host-c", &[("Fleet", "deployable")]).await;

    let after = list_with(&emulator, &first, &json!({})).await;
    assert_eq!(
        after, before,
        "case {:?}: expected the past deployment's instances to be unchanged, got {after:?}",
        "the past deployment after the fleet changed"
    );

    let second = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &second, "after the fleet changed").await;
    let fresh = list_with(&emulator, &second, &json!({})).await;
    assert_eq!(
        fresh,
        vec!["host-b".to_string(), "host-c".to_string()],
        "case {:?}: expected the fleet as it stands now, got {fresh:?}",
        "after the fleet changed"
    );
}

// ---------------------------------------------------------------------------
// The filters
// ---------------------------------------------------------------------------

/// RULE: a filter keeps an instance only if the property it selects on holds one
/// of the values it lists, and a request carrying both filters keeps the
/// instances that satisfy each; a filter listing no value names no instance, and
/// a filter the request omits narrows nothing.
#[tokio::test]
async fn a_filter_keeps_the_instances_whose_property_it_names() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    // A blue/green deployment, so BOTH filters are ones this deployment can be
    // asked about; an `instanceTypeFilter` against an in-place deployment is a
    // refusal, pinned below.
    group_with(&emulator, "blue-green-app", "Server", &blue_green_selector()).await;
    let deployment_id = deploy(&emulator, "blue-green-app").await;
    terminal_deployment(&emulator, &deployment_id, "filters").await;

    let both = ["host-a", "host-b"];
    let none: [&str; 0] = [];
    // Rows differ along which filter is present, along the number and the
    // membership of the values it lists, along a value's case, and along whether
    // the instances have the property's value at all.
    let cases: &[(&str, Value, &[&str])] = &[
        ("no filter member at all", json!({}), &both),
        (
            "the status the finished deployment gave its instances",
            json!({"instanceStatusFilter": ["Succeeded"]}),
            &both,
        ),
        (
            "a status none of the instances holds",
            json!({"instanceStatusFilter": ["Failed"]}),
            &none,
        ),
        (
            "several statuses, one of which matches",
            json!({"instanceStatusFilter": ["Pending", "InProgress", "Succeeded"]}),
            &both,
        ),
        (
            "several statuses, none of which matches",
            json!({"instanceStatusFilter": ["Pending", "InProgress", "Skipped"]}),
            &none,
        ),
        (
            "a status filter listing no value at all",
            json!({"instanceStatusFilter": []}),
            &none,
        ),
        (
            "the environment of the original instances of a blue/green deployment",
            json!({"instanceTypeFilter": ["Blue"]}),
            &both,
        ),
        (
            "the environment this service provisions no replacement instance of",
            json!({"instanceTypeFilter": ["Green"]}),
            &none,
        ),
        (
            "both environments",
            json!({"instanceTypeFilter": ["Blue", "Green"]}),
            &both,
        ),
        (
            "an environment filter listing no value at all",
            json!({"instanceTypeFilter": []}),
            &none,
        ),
        (
            "a status and an environment, both of which the instances hold",
            json!({"instanceStatusFilter": ["Succeeded"], "instanceTypeFilter": ["Blue"]}),
            &both,
        ),
        (
            "a status and an environment, only one of which the instances hold",
            json!({"instanceStatusFilter": ["Failed"], "instanceTypeFilter": ["Blue"]}),
            &none,
        ),
        (
            "a status and an environment, only the other of which the instances hold",
            json!({"instanceStatusFilter": ["Succeeded"], "instanceTypeFilter": ["Green"]}),
            &none,
        ),
    ];

    for (case, extras, expected) in cases {
        let got = list_with(&emulator, &deployment_id, extras).await;
        assert_eq!(
            got, *expected,
            "case {case:?}: expected the instances {expected:?}, got {got:?}"
        );
    }
}

/// RULE: every instance of a deployment carries the `InstanceStatus` the
/// deployment's own progress gives it, and a deployment that has not started has
/// no instance state to report at all — that is its
/// `DeploymentNotStartedException`, not an empty page.
#[tokio::test]
async fn an_instances_status_is_the_one_its_deployments_progress_implies() {
    // Rows span the model's `DeploymentStatus`
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
    // InProgress | Baking | Succeeded | Failed | Stopped | Ready): the status the
    // deployment holds, whether it has started, and the `InstanceStatus` its
    // instances then hold — or `None` where the deployment has not started and
    // the call is refused.
    let cases: &[(&str, bool, Option<&str>)] = &[
        ("Created", false, None),
        ("Queued", false, None),
        ("InProgress", true, Some("InProgress")),
        ("Baking", true, Some("InProgress")),
        ("Ready", true, Some("InProgress")),
        ("Succeeded", true, Some("Succeeded")),
        ("Failed", true, Some("Failed")),
        ("Stopped", true, Some("Skipped")),
    ];

    for (status, started, expected) in cases {
        let emulator = Emulator::new();
        register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
        let application = "status-app";
        group_with(&emulator, application, "Server", &fleet_selector()).await;
        let deployment_id = deploy(&emulator, application).await;
        force_status(&emulator, &deployment_id, status, *started);

        let Some(expected_status) = expected else {
            let (code, body) = emulator
                .call(
                    "ListDeploymentInstances",
                    json!({"deploymentId": deployment_id}),
                )
                .await;
            assert_eq!(
                (code, error_code(&body).as_str()),
                (400, "DeploymentNotStartedException"),
                "case {status:?}: expected the not-started refusal, got {code} {body:?}"
            );
            continue;
        };

        // The status the instances hold keeps them, and every other status the
        // model carries keeps none of them. The catalogue is the GENERATED one
        // this crate renders from the model's `InstanceStatus`, so `Unknown` and
        // `Ready`, which no row above names, are covered here.
        for candidate in codedeploy::ops::types::INSTANCE_STATUS_VALUES {
            let got = list_with(
                &emulator,
                &deployment_id,
                &json!({"instanceStatusFilter": [candidate]}),
            )
            .await;
            let expected_ids: &[&str] = if candidate == expected_status {
                &["host-a"]
            } else {
                &[]
            };
            assert_eq!(
                got, expected_ids,
                "case {status:?} filtered on {candidate:?}: expected {expected_ids:?}, got {got:?}"
            );
        }
    }
}

/// RULE: every value the model's own enums carry for a filter is ACCEPTED — the
/// refusals below are about values outside them, not about filtering itself.
#[tokio::test]
async fn every_modelled_filter_value_is_accepted() {
    let emulator = Emulator::new();
    let application = "accepted-app";
    // A blue/green group, so a filter on the environment an instance belongs to
    // is one this deployment can be asked about at all.
    group_with(&emulator, application, "Server", &blue_green_selector()).await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "accepted").await;

    // The catalogues are the GENERATED ones this crate renders from the model —
    // `ops::types::INSTANCE_STATUS_VALUES` and `INSTANCE_TYPE_VALUES`.
    for status in codedeploy::ops::types::INSTANCE_STATUS_VALUES {
        let (code, body) = emulator
            .call(
                "ListDeploymentInstances",
                json!({
                    "deploymentId": deployment_id,
                    "instanceStatusFilter": [status],
                }),
            )
            .await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected the modelled status to be accepted, got {code} {body:?}"
        );
    }
    for instance_type in codedeploy::ops::types::INSTANCE_TYPE_VALUES {
        let (code, body) = emulator
            .call(
                "ListDeploymentInstances",
                json!({
                    "deploymentId": deployment_id,
                    "instanceTypeFilter": [instance_type],
                }),
            )
            .await;
        assert_eq!(
            code, 200,
            "case {instance_type:?}: expected the modelled instance type to be accepted, got \
             {code} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// RULE: a listing answers at most one page of instances, with the token that
/// continues it, and the pages read in order are the whole resolution order
/// exactly once; the last page carries no token.
#[tokio::test]
async fn the_instances_are_answered_one_page_at_a_time() {
    let emulator = Emulator::new();
    let page = codedeploy::state::PAGE_SIZE;
    // One page and a bit, so the first answer is full and carries a token and
    // the second is short and carries none.
    let total = page + 20;
    let fleet: Vec<String> = (0..total).map(|index| format!("host-{index:04}")).collect();
    for instance in &fleet {
        register(&emulator, instance, &[("Fleet", "deployable")]).await;
    }
    let application = "paged-app";
    group_with(&emulator, application, "Server", &fleet_selector()).await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "paged").await;

    let first = emulator
        .ok(
            "ListDeploymentInstances",
            json!({"deploymentId": deployment_id}),
        )
        .await;
    let first_ids = listed(&first);
    assert_eq!(
        first_ids.len(),
        page,
        "case {:?}: expected a full page of {page} instances, got {}",
        "first page",
        first_ids.len()
    );
    let token = first["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case {:?}: expected a continuation token, got {first:?}",
        "first page"
    );

    let second = emulator
        .ok(
            "ListDeploymentInstances",
            json!({"deploymentId": deployment_id, "nextToken": token}),
        )
        .await;
    let second_ids = listed(&second);
    assert_eq!(
        second_ids.len(),
        total - page,
        "case {:?}: expected the remaining {} instances, got {}",
        "second page",
        total - page,
        second_ids.len()
    );
    assert!(
        second["nextToken"].is_null(),
        "case {:?}: expected no token on the last page, got {second:?}",
        "second page"
    );

    let walked: Vec<String> = first_ids.into_iter().chain(second_ids).collect();
    assert_eq!(
        walked, fleet,
        "case {:?}: expected the pages to walk the whole resolution order once, got {walked:?}",
        "both pages"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request this operation cannot answer is refused with the modelled
/// error the model gives it for that cause, and HTTP 400.
#[tokio::test]
async fn a_request_that_cannot_be_answered_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    // An in-place deployment, whose instances belong to no environment, and an
    // Amazon ECS one, whose targets are not instances at all.
    group_with(&emulator, "refusal-app", "Server", &fleet_selector()).await;
    let in_place = deploy(&emulator, "refusal-app").await;
    group_with(
        &emulator,
        "ecs-app",
        "ECS",
        &json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
    )
    .await;
    let ecs = deploy(&emulator, "ecs-app").await;
    terminal_deployment(&emulator, &in_place, "refusals").await;
    terminal_deployment(&emulator, &ecs, "refusals on ecs").await;

    // Rows differ along WHICH part of the request is unanswerable: the
    // deployment id (absent, malformed, or naming no deployment), the compute
    // platform of the deployment it names, a filter value, the environment
    // filter against a deployment with one environment, and the continuation
    // token.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an empty deployment id",
            json!({"deploymentId": ""}),
            "DeploymentIdRequiredException",
        ),
        (
            "a blank deployment id",
            json!({"deploymentId": "   "}),
            "DeploymentIdRequiredException",
        ),
        (
            "a deployment id of no recognisable form",
            json!({"deploymentId": "not-an-id"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a deployment id with too short a suffix",
            json!({"deploymentId": "d-SHORT"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a deployment id whose suffix is lowercase",
            json!({"deploymentId": "d-abcdefghi"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a well-formed deployment id no deployment has",
            json!({"deploymentId": "d-ZZZZZZZZZ"}),
            "DeploymentDoesNotExistException",
        ),
        // The model states the restriction on this operation itself:
        // "ListDeploymentInstances throws an exception if it is used with a
        // compute platform other than EC2/On-premises or Lambda".
        (
            "a deployment on the Amazon ECS compute platform",
            json!({"deploymentId": ecs}),
            "InvalidComputePlatformException",
        ),
        (
            "an Amazon ECS deployment asked with a filter as well",
            json!({"deploymentId": ecs, "instanceStatusFilter": ["Succeeded"]}),
            "InvalidComputePlatformException",
        ),
        (
            "a status no instance ever holds",
            json!({"deploymentId": in_place, "instanceStatusFilter": ["Bogus"]}),
            "InvalidInstanceStatusException",
        ),
        (
            "a status in the wrong case",
            json!({"deploymentId": in_place, "instanceStatusFilter": ["succeeded"]}),
            "InvalidInstanceStatusException",
        ),
        (
            "one good status and one that is no status",
            json!({"deploymentId": in_place, "instanceStatusFilter": ["Succeeded", "Bogus"]}),
            "InvalidInstanceStatusException",
        ),
        (
            "an instance type that names neither environment",
            json!({"deploymentId": in_place, "instanceTypeFilter": ["Purple"]}),
            "InvalidInstanceTypeException",
        ),
        (
            "an instance type in the wrong case",
            json!({"deploymentId": in_place, "instanceTypeFilter": ["blue"]}),
            "InvalidInstanceTypeException",
        ),
        (
            "a continuation token this service did not issue",
            json!({"deploymentId": in_place, "nextToken": "not-a-token"}),
            "InvalidNextTokenException",
        ),
        // Selecting by the environment an instance belongs to against an
        // IN-PLACE deployment, which has only one environment: the modelled
        // `InvalidDeploymentInstanceTypeException`, whatever the filter lists.
        (
            "the original environment of an in-place deployment",
            json!({"deploymentId": in_place, "instanceTypeFilter": ["Blue"]}),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "the replacement environment of an in-place deployment",
            json!({"deploymentId": in_place, "instanceTypeFilter": ["Green"]}),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "an environment filter listing no value, on an in-place deployment",
            json!({"deploymentId": in_place, "instanceTypeFilter": []}),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "an environment filter beside a status one, on an in-place deployment",
            json!({
                "deploymentId": in_place,
                "instanceStatusFilter": ["Succeeded"],
                "instanceTypeFilter": ["Blue"],
            }),
            "InvalidDeploymentInstanceTypeException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("ListDeploymentInstances", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected a 400 {expected}, got {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected the refusal to carry a message, got {body:?}"
        );
    }
}

/// RULE: listing a deployment's instances CHANGES nothing a later caller sees —
/// it is a report of the deployment's own record.
#[tokio::test]
async fn listing_the_instances_changes_nothing() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    let application = "read-only-app";
    group_with(&emulator, application, "Server", &fleet_selector()).await;
    let deployment_id = deploy(&emulator, application).await;
    let before = terminal_deployment(&emulator, &deployment_id, "read-only").await;

    let first = list_with(&emulator, &deployment_id, &json!({})).await;
    for _ in 0..3 {
        let again = list_with(&emulator, &deployment_id, &json!({})).await;
        assert_eq!(
            again, first,
            "case {:?}: expected every read to answer the same instances, got {again:?}",
            "repeated reads"
        );
    }
    let after = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone();
    assert_eq!(
        after, before,
        "case {:?}: expected the deployment to be untouched by the listing, got {after:?}",
        "the deployment after the reads"
    );
}
