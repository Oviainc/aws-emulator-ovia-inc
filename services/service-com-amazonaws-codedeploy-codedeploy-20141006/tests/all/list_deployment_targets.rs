// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentTargets` over the wire.
//!
//! RULE: the call answers the ids of the deployment targets the deployment's own
//! record holds — the snapshot its deployment group resolved when it was created
//! — in resolution order, narrowed by the `targetFilters` the request names, one
//! page of `state::PAGE_SIZE` at a time; a deployment that has not started is
//! refused with `DeploymentNotStartedException`, a filter on the environment a
//! target belongs to is refused on an in-place deployment with
//! `InvalidDeploymentInstanceTypeException`, and a filter naming a property or a
//! value no target has is refused with the error the model gives this operation
//! for it.
//!
//! The harness — the router, the `X-Amz-Target` request, the error-code helper
//! and the terminal-status poll — is the one `codedeploy_lifecycle` drives the
//! rest of this service with; only the cases and the fleet helpers here are new.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, s3_revision, terminal_deployment, Emulator};

/// The account the IAM ARNs these cases register on-premises instances with
/// name, and the role their deployment groups carry; CodeDeploy only stores
/// both.
const ACCOUNT: &str = "123456789012";
/// See [`ACCOUNT`].
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The one deployment group name every case here creates.
const GROUP: &str = "dg";

/// Register an on-premises instance carrying `tags`.
///
/// `pub(crate)` because the `deployment_targets` suite drives the same fleet
/// through the readers of a single target; there is one set of fleet helpers for
/// the target suites, not one per suite.
pub(crate) async fn register(emulator: &Emulator, instance: &str, tags: &[(&str, &str)]) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": instance,
                "iamUserArn": format!("arn:aws:iam::{ACCOUNT}:user/{instance}"),
            }),
        )
        .await;
    if tags.is_empty() {
        return;
    }
    let tags: Vec<Value> = tags
        .iter()
        .map(|(key, value)| json!({"Key": key, "Value": value}))
        .collect();
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": [instance], "tags": tags}),
        )
        .await;
}

/// Create an application on `platform` and a deployment group of it carrying
/// `extras`.
pub(crate) async fn group_with(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extras: &Value,
) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": GROUP,
        "serviceRoleArn": SERVICE_ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
}

/// Deploy a revision to the one deployment group of `application`, answering the
/// deployment id.
pub(crate) async fn deploy(emulator: &Emulator, application: &str) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": GROUP,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .expect("CreateDeployment answers a deployment id")
        .to_string()
}

/// The `targetIds` of a successful `ListDeploymentTargets` answer.
fn listed(answer: &Value) -> Vec<String> {
    answer["targetIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// List the targets of `deployment_id`, with no filter.
async fn list_all(emulator: &Emulator, deployment_id: &str) -> Vec<String> {
    listed(
        &emulator
            .ok(
                "ListDeploymentTargets",
                json!({"deploymentId": deployment_id}),
            )
            .await,
    )
}

/// Put the stored deployment into `status`, as far along as `started` says, and
/// hold the clock off it so it stays there.
///
/// The statuses a deployment reaches on its own are a few of the model's
/// `DeploymentStatus` enum; the rest (`Queued`, `Baking`, `Ready`) are reached
/// by real CodeDeploy and by no call this emulator serves, so a case that needs
/// one writes it into the very store the handlers read — `state::service_state`,
/// never a private engine — exactly as `github_account_tokens` seeds a
/// connection no API creates. The due instants are pushed out of reach so the
/// clock, which every request advances, does not move the record off the status
/// under test.
pub(crate) fn force_status(
    emulator: &Emulator,
    deployment_id: &str,
    status: &str,
    started: bool,
) {
    let state = codedeploy::state::service_state(&*emulator.engine)
        .expect("the shared in-memory store is reachable");
    let mut guard = state.lock();
    let deployment = guard
        .deployments
        .get_mut(deployment_id)
        .expect("the deployment the case created is stored");
    let out_of_reach = deployment.create_time_ms + 3_600_000;
    deployment.status = status.to_string();
    deployment.start_time_ms = started.then_some(deployment.create_time_ms);
    deployment.complete_time_ms = None;
    deployment.starts_at_ms = out_of_reach;
    deployment.completes_at_ms = out_of_reach;
}

// ---------------------------------------------------------------------------
// Which targets a deployment holds
// ---------------------------------------------------------------------------

/// RULE: the listed ids are the targets the deployment's group resolved, under
/// the id CodeDeploy reports for a target of that compute platform
/// (`smithy_query(query="doc",shape="BatchGetDeploymentTargetsInput",member="targetIds")`:
/// an instance id for EC2/on-premises, a `<clustername>:<servicename>` pair for
/// Amazon ECS), in resolution order.
#[tokio::test]
async fn the_listed_ids_are_the_targets_the_deployment_group_resolved() {
    // Rows differ along the compute platform, along the SELECTOR the group
    // resolves its targets with, and along how many of the registered instances
    // that selector takes in. The fleet is `(instance name, tags)`, registered
    // in order; a row's expectation is the ids in resolution order.
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
            case: "an on-premises tag filter every tagged instance matches",
            platform: "Server",
            extras: json!({
                "onPremisesInstanceTagFilters": [
                    {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
                ]
            }),
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
            case: "an on-premises tag filter on the key alone",
            platform: "Server",
            extras: json!({
                "onPremisesInstanceTagFilters": [{"Key": "Fleet", "Type": "KEY_ONLY"}]
            }),
            fleet,
            expected: &["host-a", "host-b", "host-c"],
        },
        Case {
            case: "an on-premises tag set both of whose groups must match",
            platform: "Server",
            extras: json!({
                "onPremisesTagSet": {"onPremisesTagSetList": [
                    [{"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"}],
                    [{"Key": "Rack", "Value": "one", "Type": "KEY_AND_VALUE"}],
                ]}
            }),
            fleet,
            expected: &["host-a"],
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
            case: "an auto scaling group, which names inventory this service keeps none of",
            platform: "Server",
            extras: json!({"autoScalingGroups": ["asg-web"]}),
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
        Case {
            case: "two ecs services",
            platform: "ECS",
            extras: json!({
                "ecsServices": [
                    {"clusterName": "prod", "serviceName": "web"},
                    {"clusterName": "prod", "serviceName": "api"},
                ]
            }),
            fleet: &[],
            expected: &["prod:web", "prod:api"],
        },
        Case {
            case: "one ecs service",
            platform: "ECS",
            extras: json!({"ecsServices": [{"clusterName": "staging", "serviceName": "web"}]}),
            fleet: &[],
            expected: &["staging:web"],
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

        let got = list_all(&emulator, &deployment_id).await;
        assert_eq!(
            got, case.expected,
            "case {:?}: expected the targets {:?}, got {got:?}",
            case.case, case.expected
        );
    }
}

/// RULE: the targets a deployment lists are the ones its group resolved WHEN it
/// was created — deregistering an instance or retargeting the group afterwards
/// does not change what a past deployment deployed to — and a deregistered
/// instance is no target of a deployment created after it left the fleet.
#[tokio::test]
async fn the_listing_is_the_fleet_the_deployment_resolved_when_it_was_created() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    let application = "snapshot-app";
    group_with(
        &emulator,
        application,
        "Server",
        &json!({
            "onPremisesInstanceTagFilters": [
                {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
            ]
        }),
    )
    .await;

    let first = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &first, "before the fleet changed").await;
    let before = list_all(&emulator, &first).await;
    assert_eq!(
        before,
        vec!["host-a".to_string(), "host-b".to_string()],
        "case {:?}: expected both registered instances, got {before:?}",
        "before the fleet changed"
    );

    // The fleet changes under the finished deployment: one instance leaves it,
    // and one that could never have been part of it joins.
    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "host-a"}),
        )
        .await;
    register(&emulator, "host-c", &[("Fleet", "deployable")]).await;

    let after = list_all(&emulator, &first).await;
    assert_eq!(
        after, before,
        "case {:?}: expected the past deployment's targets to be unchanged, got {after:?}",
        "the past deployment after the fleet changed"
    );

    // A deployment created now resolves the fleet as it is now.
    let second = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &second, "after the fleet changed").await;
    let fresh = list_all(&emulator, &second).await;
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

/// RULE: a `targetFilters` entry keeps a target only if the property its name
/// selects holds one of the values it lists, and a request carrying several
/// entries keeps the targets that satisfy EVERY one of them. A target's
/// `TargetStatus` follows how far its deployment has got; its
/// `ServerInstanceLabel` is the environment it belongs to, which only a
/// blue/green deployment's targets have (filtering an in-place deployment by it
/// is refused, with the refusals below).
#[tokio::test]
async fn a_target_filter_keeps_the_targets_whose_property_it_names() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-b", &[("Fleet", "deployable")]).await;
    let selector = json!({
        "onPremisesInstanceTagFilters": [
            {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
        ]
    });

    // Two deployments of the same fleet, differing in the one thing a label
    // depends on: an in-place deployment has one environment, a blue/green one
    // has an original (`Blue`) and a replacement (`Green`) environment.
    let mut in_place_extras = selector.clone();
    in_place_extras["deploymentStyle"] =
        json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"});
    group_with(&emulator, "in-place-app", "Server", &in_place_extras).await;
    let in_place = deploy(&emulator, "in-place-app").await;

    let mut blue_green_extras = selector.clone();
    blue_green_extras["deploymentStyle"] =
        json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"});
    group_with(&emulator, "blue-green-app", "Server", &blue_green_extras).await;
    let blue_green = deploy(&emulator, "blue-green-app").await;

    terminal_deployment(&emulator, &in_place, "in-place").await;
    terminal_deployment(&emulator, &blue_green, "blue/green").await;

    let both = ["host-a", "host-b"];
    let none: [&str; 0] = [];
    // Rows differ along the filter name, along the number and the membership of
    // the values it lists, along a value's case, and along whether the
    // deployment's targets have the property at all.
    let cases: &[(&str, &str, Value, &[&str])] = &[
        ("no filter member at all", &in_place, Value::Null, &both),
        (
            "an empty filter map",
            &in_place,
            json!({}),
            &both,
        ),
        (
            "the status the finished deployment gave its targets",
            &in_place,
            json!({"TargetStatus": ["Succeeded"]}),
            &both,
        ),
        (
            "a status none of the targets holds",
            &in_place,
            json!({"TargetStatus": ["Failed"]}),
            &none,
        ),
        (
            "several statuses, one of which matches",
            &in_place,
            json!({"TargetStatus": ["Pending", "InProgress", "Succeeded"]}),
            &both,
        ),
        (
            "several statuses, none of which matches",
            &in_place,
            json!({"TargetStatus": ["Pending", "InProgress", "Skipped"]}),
            &none,
        ),
        (
            "a filter listing no value at all",
            &in_place,
            json!({"TargetStatus": []}),
            &none,
        ),
        (
            "the label of the original environment of a blue/green deployment",
            &blue_green,
            json!({"ServerInstanceLabel": ["Blue"]}),
            &both,
        ),
        (
            "the label of a replacement environment this service provisions none of",
            &blue_green,
            json!({"ServerInstanceLabel": ["Green"]}),
            &none,
        ),
        (
            "both labels",
            &blue_green,
            json!({"ServerInstanceLabel": ["Blue", "Green"]}),
            &both,
        ),
        (
            "a status and a label, both of which the targets hold",
            &blue_green,
            json!({"TargetStatus": ["Succeeded"], "ServerInstanceLabel": ["Blue"]}),
            &both,
        ),
        (
            "a status and a label, only one of which the targets hold",
            &blue_green,
            json!({"TargetStatus": ["Failed"], "ServerInstanceLabel": ["Blue"]}),
            &none,
        ),
        (
            "a status and a label, only the other of which the targets hold",
            &blue_green,
            json!({"TargetStatus": ["Succeeded"], "ServerInstanceLabel": ["Green"]}),
            &none,
        ),
    ];

    for (case, deployment_id, filters, expected) in cases {
        let mut request = json!({"deploymentId": deployment_id});
        if !filters.is_null() {
            request["targetFilters"] = filters.clone();
        }
        let got = listed(&emulator.ok("ListDeploymentTargets", request).await);
        assert_eq!(
            got, *expected,
            "case {case:?}: expected the targets {expected:?}, got {got:?}"
        );
    }
}

/// RULE: every target of a deployment carries the `TargetStatus` the
/// deployment's own progress gives it, and a deployment that has not started has
/// no target state to report at all — that is its
/// `DeploymentNotStartedException`, not an empty page.
#[tokio::test]
async fn a_targets_status_is_the_one_its_deployments_progress_implies() {
    // Rows span the model's `DeploymentStatus`
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
    // InProgress | Baking | Succeeded | Failed | Stopped | Ready): the status the
    // deployment holds, whether it has started, and the `TargetStatus` its
    // targets then hold — or `None` where the deployment has not started and the
    // call is refused.
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
        group_with(
            &emulator,
            application,
            "Server",
            &json!({
                "onPremisesInstanceTagFilters": [
                    {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
                ]
            }),
        )
        .await;
        let deployment_id = deploy(&emulator, application).await;
        force_status(&emulator, &deployment_id, status, *started);

        let Some(expected_status) = expected else {
            let (code, body) = emulator
                .call(
                    "ListDeploymentTargets",
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

        // The status the targets hold keeps them, and every other status the
        // model carries keeps none of them.
        for candidate in codedeploy::ops::types::TARGET_STATUS_VALUES {
            let got = listed(
                &emulator
                    .ok(
                        "ListDeploymentTargets",
                        json!({
                            "deploymentId": deployment_id,
                            "targetFilters": {"TargetStatus": [candidate]},
                        }),
                    )
                    .await,
            );
            let expected_ids: &[&str] = if candidate == expected_status {
                &["host-a"]
            } else {
                &[]
            };
            assert_eq!(
                got, expected_ids,
                "case {status:?} filtered on {candidate:?}: expected {expected_ids:?}, got \
                 {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// RULE: a listing answers at most one page of targets, with the token that
/// continues it, and the pages read in order are the whole resolution order
/// exactly once; the last page carries no token.
#[tokio::test]
async fn the_targets_are_answered_one_page_at_a_time() {
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
    group_with(
        &emulator,
        application,
        "Server",
        &json!({
            "onPremisesInstanceTagFilters": [
                {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
            ]
        }),
    )
    .await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "paged").await;

    let first = emulator
        .ok(
            "ListDeploymentTargets",
            json!({"deploymentId": deployment_id}),
        )
        .await;
    let first_ids = listed(&first);
    assert_eq!(
        first_ids.len(),
        page,
        "case {:?}: expected a full page of {page} targets, got {}",
        "first page",
        first_ids.len()
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case {:?}: expected a continuation token, got {first:?}",
        "first page"
    );

    let second = emulator
        .ok(
            "ListDeploymentTargets",
            json!({"deploymentId": deployment_id, "nextToken": token}),
        )
        .await;
    let second_ids = listed(&second);
    assert_eq!(
        second_ids.len(),
        total - page,
        "case {:?}: expected the remaining {} targets, got {}",
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
    let application = "refusal-app";
    group_with(
        &emulator,
        application,
        "Server",
        &json!({
            "onPremisesInstanceTagFilters": [
                {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
            ]
        }),
    )
    .await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "refusals").await;

    // Rows differ along WHICH part of the request is unanswerable: the
    // deployment id (absent, malformed, or naming no deployment), the filter
    // name, a filter value, and the continuation token.
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
        (
            "a filter name that is no target property",
            json!({"deploymentId": deployment_id, "targetFilters": {"Nope": ["Succeeded"]}}),
            "InvalidTargetFilterNameException",
        ),
        (
            "a filter name in the wrong case",
            json!({"deploymentId": deployment_id, "targetFilters": {"targetStatus": ["Succeeded"]}}),
            "InvalidTargetFilterNameException",
        ),
        (
            "a status no target ever holds",
            json!({"deploymentId": deployment_id, "targetFilters": {"TargetStatus": ["Bogus"]}}),
            "InvalidInstanceStatusException",
        ),
        (
            "a status in the wrong case",
            json!({"deploymentId": deployment_id, "targetFilters": {"TargetStatus": ["succeeded"]}}),
            "InvalidInstanceStatusException",
        ),
        (
            "one good status and one that is no status",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"TargetStatus": ["Succeeded", "Bogus"]},
            }),
            "InvalidInstanceStatusException",
        ),
        (
            "a label that names neither environment",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"ServerInstanceLabel": ["Purple"]},
            }),
            "InvalidInstanceTypeException",
        ),
        (
            "a label in the wrong case",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"ServerInstanceLabel": ["blue"]},
            }),
            "InvalidInstanceTypeException",
        ),
        (
            "a continuation token this service did not issue",
            json!({"deploymentId": deployment_id, "nextToken": "not-a-token"}),
            "InvalidNextTokenException",
        ),
        // Selecting by the environment a target belongs to against an IN-PLACE
        // deployment, which has only one environment: the modelled
        // `InvalidDeploymentInstanceTypeException` ("An instance type was
        // specified for an in-place deployment. Instance types are supported for
        // blue/green deployments only"), whatever the filter lists.
        (
            "the original environment of an in-place deployment",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"ServerInstanceLabel": ["Blue"]},
            }),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "the replacement environment of an in-place deployment",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"ServerInstanceLabel": ["Green"]},
            }),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "an environment filter listing no value, on an in-place deployment",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {"ServerInstanceLabel": []},
            }),
            "InvalidDeploymentInstanceTypeException",
        ),
        (
            "an environment filter beside a status one, on an in-place deployment",
            json!({
                "deploymentId": deployment_id,
                "targetFilters": {
                    "TargetStatus": ["Succeeded"],
                    "ServerInstanceLabel": ["Blue"],
                },
            }),
            "InvalidDeploymentInstanceTypeException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("ListDeploymentTargets", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected a 400 {expected}, got {status} {body:?}"
        );
        assert!(
            body["message"].as_str().is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected the refusal to carry a message, got {body:?}"
        );
    }
}

/// RULE: every value the model's own enums carry for a filter's property is
/// ACCEPTED — the refusals above are about values outside them, not about
/// filtering itself.
#[tokio::test]
async fn every_modelled_filter_value_is_accepted() {
    let emulator = Emulator::new();
    let application = "accepted-app";
    // A blue/green group, so a filter on the environment a target belongs to is
    // one this deployment can be asked about at all.
    group_with(
        &emulator,
        application,
        "Server",
        &json!({
            "deploymentStyle": {
                "deploymentType": "BLUE_GREEN",
                "deploymentOption": "WITH_TRAFFIC_CONTROL",
            }
        }),
    )
    .await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "accepted").await;

    // The catalogues are the GENERATED ones this crate renders from the model —
    // `ops::types::TARGET_STATUS_VALUES` and `TARGET_LABEL_VALUES` — so
    // `Unknown` and `Ready`, which no case above names, are covered here.
    for status in codedeploy::ops::types::TARGET_STATUS_VALUES {
        let (code, body) = emulator
            .call(
                "ListDeploymentTargets",
                json!({
                    "deploymentId": deployment_id,
                    "targetFilters": {"TargetStatus": [status]},
                }),
            )
            .await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected the modelled status to be accepted, got {code} {body:?}"
        );
    }
    for label in codedeploy::ops::types::TARGET_LABEL_VALUES {
        let (code, body) = emulator
            .call(
                "ListDeploymentTargets",
                json!({
                    "deploymentId": deployment_id,
                    "targetFilters": {"ServerInstanceLabel": [label]},
                }),
            )
            .await;
        assert_eq!(
            code, 200,
            "case {label:?}: expected the modelled label to be accepted, got {code} {body:?}"
        );
    }
}
