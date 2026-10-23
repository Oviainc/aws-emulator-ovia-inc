// SPDX-License-Identifier: CC0-1.0

//! `BatchGetDeploymentTargets` and `GetDeploymentTarget` over the wire.
//!
//! RULE: both readers answer the target the deployment's own record holds under
//! the id asked for — the snapshot its deployment group resolved when it was
//! created — as the `DeploymentTarget` member its `DeploymentTargetType` names,
//! carrying the deployment's id, the target's id and label, the `TargetStatus`
//! the deployment's progress gives it and the instant that status last changed;
//! the batch reader answers one entry per id in the order the request named
//! them, and both refuse an id the deployment holds no target under, an id
//! outside the form a target of that compute platform is reported under, and a
//! deployment that has not started.
//!
//! The harness — the router, the `X-Amz-Target` request and the error-code
//! helper — is the one `codedeploy_lifecycle` drives the rest of this service
//! with, and the fleet helpers are `list_deployment_targets`'; only the cases
//! here are new.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, terminal_deployment, Emulator};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// The `DeploymentTarget` member a target of each `DeploymentTargetType` is
/// carried in, from the model: `DeploymentTarget` has one member per target type
/// (`smithy_query(query="shape",shape="DeploymentTarget")`: instanceTarget,
/// lambdaTarget, ecsTarget, cloudFormationTarget) and `DeploymentTargetType`
/// names which (`smithy_query(query="shape",shape="DeploymentTargetType")`:
/// InstanceTarget | LambdaTarget | ECSTarget | CloudFormationTarget).
const TARGET_MEMBERS: &[(&str, &str)] = &[
    ("InstanceTarget", "instanceTarget"),
    ("LambdaTarget", "lambdaTarget"),
    ("ECSTarget", "ecsTarget"),
    ("CloudFormationTarget", "cloudFormationTarget"),
];

/// An on-premises selector every instance tagged `Fleet=deployable` satisfies.
fn deployable() -> Value {
    json!({
        "onPremisesInstanceTagFilters": [
            {"Key": "Fleet", "Value": "deployable", "Type": "KEY_AND_VALUE"},
        ]
    })
}

/// The `deploymentTargets` of a successful `BatchGetDeploymentTargets` answer.
async fn batch(emulator: &Emulator, deployment_id: &str, target_ids: &[&str]) -> Vec<Value> {
    emulator
        .ok(
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": target_ids}),
        )
        .await["deploymentTargets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `deploymentTarget` of a successful `GetDeploymentTarget` answer.
async fn get_one(emulator: &Emulator, deployment_id: &str, target_id: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": target_id}),
        )
        .await["deploymentTarget"]
        .clone()
}

/// The one per-type member a `DeploymentTarget` carries, and the type it says it
/// is: every other per-type member must be absent, so a target is reported in
/// exactly one of them.
fn carried(case: &str, target: &Value) -> (String, Value) {
    let reported = target["deploymentTargetType"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expected_member = TARGET_MEMBERS
        .iter()
        .find(|(kind, _)| *kind == reported)
        .map(|(_, member)| *member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a modelled target type, got {target:?}"));
    for (kind, member) in TARGET_MEMBERS {
        let present = !target[*member].is_null();
        assert_eq!(
            present,
            *member == expected_member,
            "case {case:?}: expected only the {expected_member} member of a {kind} target, got \
             {target:?}"
        );
    }
    (reported, target[expected_member].clone())
}

/// The ARN the on-premises registry minted for `instance`.
async fn registered_arn(emulator: &Emulator, instance: &str) -> String {
    emulator
        .ok(
            "GetOnPremisesInstance",
            json!({"instanceName": instance}),
        )
        .await["instanceInfo"]["instanceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// What a target reads back as
// ---------------------------------------------------------------------------

/// RULE: each reader answers the target the deployment resolved, in the
/// `DeploymentTarget` member its target type names, with the deployment's id,
/// the target's own id and environment label, the status the deployment's
/// progress gives it and the instant that status last changed — and the two
/// readers answer the SAME document for the same target.
#[tokio::test]
async fn a_resolved_target_reads_back_the_same_way_from_both_readers() {
    // Rows differ along the compute platform (so along the target type and the
    // id FORM the platform reports), along the deployment style (so along
    // whether a target belongs to a named environment) and along how many
    // targets the group resolves. `arn` says whether the target has an ARN this
    // service holds: an instance's ARN is in its own registry, while an ECS
    // service's is Amazon ECS's record, which this emulator keeps none of.
    struct Case<'a> {
        case: &'a str,
        application: &'a str,
        platform: &'a str,
        extras: Value,
        fleet: &'a [&'a str],
        expected_type: &'a str,
        expected_ids: &'a [&'a str],
        expected_label: Option<&'a str>,
        expects_arn: bool,
    }
    let cases: &[Case] = &[
        Case {
            case: "an in-place EC2/on-premises deployment of two instances",
            application: "in-place-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-a", "host-b"],
            expected_type: "InstanceTarget",
            expected_ids: &["host-a", "host-b"],
            expected_label: None,
            expects_arn: true,
        },
        Case {
            case: "an in-place EC2/on-premises deployment of one instance",
            application: "single-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-only"],
            expected_type: "InstanceTarget",
            expected_ids: &["host-only"],
            expected_label: None,
            expects_arn: true,
        },
        Case {
            case: "a blue/green deployment, whose targets are the original environment",
            application: "blue-green-app",
            platform: "Server",
            extras: {
                let mut extras = deployable();
                extras["deploymentStyle"] = json!({
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                });
                extras
            },
            fleet: &["host-blue"],
            expected_type: "InstanceTarget",
            // `TargetLabel` (`smithy_query(query="shape",shape="TargetLabel")`:
            // Blue | Green): the instances a group selects are the ORIGINAL
            // environment, which the model calls BLUE.
            expected_label: Some("Blue"),
            expected_ids: &["host-blue"],
            expects_arn: true,
        },
        Case {
            case: "an Amazon ECS deployment, whose target is a cluster/service pair",
            application: "ecs-app",
            platform: "ECS",
            extras: json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
            fleet: &[],
            expected_type: "ECSTarget",
            expected_ids: &["prod:web"],
            expected_label: None,
            expects_arn: false,
        },
    ];

    for case in cases {
        // One emulator per row: the on-premises registry is account-wide, so a
        // row's fleet is only exact over a registry of its own.
        let emulator = Emulator::new();
        for instance in case.fleet {
            register(&emulator, instance, &[("Fleet", "deployable")]).await;
        }
        group_with(&emulator, case.application, case.platform, &case.extras).await;
        let deployment_id = deploy(&emulator, case.application).await;
        terminal_deployment(&emulator, &deployment_id, case.case).await;

        let answered = batch(&emulator, &deployment_id, case.expected_ids).await;
        assert_eq!(
            answered.len(),
            case.expected_ids.len(),
            "case {:?}: expected one entry per requested id {:?}, got {answered:?}",
            case.case,
            case.expected_ids
        );

        for (index, expected_id) in case.expected_ids.iter().enumerate() {
            let target = &answered[index];
            let (reported_type, carried_target) = carried(case.case, target);
            assert_eq!(
                reported_type, case.expected_type,
                "case {:?}: expected the target type {}, got {target:?}",
                case.case, case.expected_type
            );
            assert_eq!(
                carried_target["targetId"].as_str(),
                Some(*expected_id),
                "case {:?}: expected the target id {expected_id}, got {carried_target:?}",
                case.case
            );
            assert_eq!(
                carried_target["deploymentId"].as_str(),
                Some(deployment_id.as_str()),
                "case {:?}: expected the deployment id {deployment_id}, got {carried_target:?}",
                case.case
            );
            assert_eq!(
                carried_target["status"].as_str(),
                Some("Succeeded"),
                "case {:?}: expected the status the finished deployment gives its targets, got \
                 {carried_target:?}",
                case.case
            );
            assert!(
                carried_target["lastUpdatedAt"].is_number(),
                "case {:?}: expected an instant the target last changed at, got \
                 {carried_target:?}",
                case.case
            );
            assert_eq!(
                carried_target["instanceLabel"].as_str(),
                case.expected_label,
                "case {:?}: expected the environment label {:?}, got {carried_target:?}",
                case.case,
                case.expected_label
            );
            if case.expects_arn {
                let expected_arn = registered_arn(&emulator, expected_id).await;
                assert_eq!(
                    carried_target["targetArn"].as_str(),
                    Some(expected_arn.as_str()),
                    "case {:?}: expected the ARN the registry minted ({expected_arn}), got \
                     {carried_target:?}",
                    case.case
                );
            } else {
                assert!(
                    carried_target["targetArn"].is_null(),
                    "case {:?}: expected no ARN for a resource this service keeps no record of, \
                     got {carried_target:?}",
                    case.case
                );
            }

            // The individual read of the same target is the same document.
            let fetched = get_one(&emulator, &deployment_id, expected_id).await;
            assert_eq!(
                &fetched, target,
                "case {:?}: expected GetDeploymentTarget to answer the batch entry for \
                 {expected_id}, got {fetched:?}",
                case.case
            );
        }
    }
}

/// RULE: the batch answers one entry per id the request named, in THAT order —
/// not in resolution order — however many times an id is named.
#[tokio::test]
async fn the_batch_answers_the_ids_in_the_order_the_request_named_them() {
    let emulator = Emulator::new();
    let fleet = ["host-a", "host-b", "host-c"];
    for instance in fleet {
        register(&emulator, instance, &[("Fleet", "deployable")]).await;
    }
    let application = "order-app";
    group_with(&emulator, application, "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "order").await;

    // Rows differ along the order the ids are named in, along how many of them
    // are named, and along whether one is named twice.
    let cases: &[(&str, &[&str])] = &[
        ("resolution order", &["host-a", "host-b", "host-c"]),
        ("reverse resolution order", &["host-c", "host-b", "host-a"]),
        ("a single id", &["host-b"]),
        ("a subset, out of order", &["host-c", "host-a"]),
        ("the same id twice", &["host-b", "host-b"]),
    ];

    for (case, requested) in cases {
        let answered = batch(&emulator, &deployment_id, requested).await;
        let got: Vec<String> = answered
            .iter()
            .map(|target| {
                carried(case, target).1["targetId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            got, *requested,
            "case {case:?}: expected the ids in the order requested {requested:?}, got {got:?}"
        );
    }
}

/// RULE: a target carries the `TargetStatus` its deployment's own progress gives
/// it, and a deployment that has not started has no target state to report at
/// all — both readers refuse it with `DeploymentNotStartedException`.
#[tokio::test]
async fn a_targets_status_is_the_one_its_deployments_progress_implies() {
    // Rows span the model's `DeploymentStatus`
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued
    // | InProgress | Baking | Succeeded | Failed | Stopped | Ready): the status
    // the deployment holds, whether it has started, and the `TargetStatus` its
    // targets then hold — or `None` where the deployment has not started and
    // both calls are refused.
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
        group_with(&emulator, application, "Server", &deployable()).await;
        let deployment_id = deploy(&emulator, application).await;
        force_status(&emulator, &deployment_id, status, *started);

        let Some(expected_status) = expected else {
            for operation in ["BatchGetDeploymentTargets", "GetDeploymentTarget"] {
                let request = if operation == "BatchGetDeploymentTargets" {
                    json!({"deploymentId": deployment_id, "targetIds": ["host-a"]})
                } else {
                    json!({"deploymentId": deployment_id, "targetId": "host-a"})
                };
                let (code, body) = emulator.call(operation, request).await;
                assert_eq!(
                    (code, error_code(&body).as_str()),
                    (400, "DeploymentNotStartedException"),
                    "case {status:?} on {operation}: expected the not-started refusal, got {code} \
                     {body:?}"
                );
            }
            continue;
        };

        let answered = batch(&emulator, &deployment_id, &["host-a"]).await;
        let batched = carried(status, &answered[0]).1;
        assert_eq!(
            batched["status"].as_str(),
            Some(*expected_status),
            "case {status:?}: expected the target status {expected_status}, got {batched:?}"
        );
        let fetched = get_one(&emulator, &deployment_id, "host-a").await;
        assert_eq!(
            carried(status, &fetched).1["status"].as_str(),
            Some(*expected_status),
            "case {status:?}: expected GetDeploymentTarget to report the same status, got \
             {fetched:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// RULE: a request neither reader can answer is refused with the error the model
/// gives it for that part of the request — the deployment id's absence and its
/// format before the deployment is looked up, then the target id's absence, its
/// format for the deployment's compute platform, and finally whether the
/// deployment holds a target under it.
#[tokio::test]
async fn an_unanswerable_request_is_refused_with_the_modelled_error() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    let application = "refusal-app";
    group_with(&emulator, application, "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "refusals").await;

    // 26 ids: one past the bound this operation publishes.
    let too_many: Vec<String> = (0..26).map(|index| format!("host-{index:02}")).collect();
    // A name one character longer than the service's resource-name bound
    // (`smithy_query(query="shape",shape="ApplicationName")`: @length max 100).
    let too_long = "h".repeat(101);
    let at_the_bound = "h".repeat(100);

    // Rows differ along WHICH part of the request is unanswerable, and along the
    // reader asked.
    let cases: &[(&str, &str, Value, &str)] = &[
        (
            "an empty deployment id, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": "", "targetIds": ["host-a"]}),
            "DeploymentIdRequiredException",
        ),
        (
            "a blank deployment id, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": "   ", "targetId": "host-a"}),
            "DeploymentIdRequiredException",
        ),
        (
            "a deployment id of no recognisable form, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": "not-an-id", "targetIds": ["host-a"]}),
            "InvalidDeploymentIdException",
        ),
        (
            "a deployment id whose suffix is lowercase, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": "d-abcdefghi", "targetId": "host-a"}),
            "InvalidDeploymentIdException",
        ),
        (
            "a well-formed deployment id no deployment has, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": "d-ZZZZZZZZZ", "targetIds": ["host-a"]}),
            "DeploymentDoesNotExistException",
        ),
        (
            "a well-formed deployment id no deployment has, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": "d-ZZZZZZZZZ", "targetId": "host-a"}),
            "DeploymentDoesNotExistException",
        ),
        (
            "no target id at all, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": []}),
            "DeploymentTargetIdRequiredException",
        ),
        (
            "a blank target id among good ones, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": ["host-a", "  "]}),
            "DeploymentTargetIdRequiredException",
        ),
        (
            "an empty target id, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": ""}),
            "DeploymentTargetIdRequiredException",
        ),
        (
            "one id past the published bound, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": too_many}),
            "DeploymentTargetListSizeExceededException",
        ),
        (
            "a well-formed id this deployment has no target under, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": ["host-a", "host-z"]}),
            "DeploymentTargetDoesNotExistException",
        ),
        (
            "a well-formed id this deployment has no target under, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": "host-z"}),
            "DeploymentTargetDoesNotExistException",
        ),
        (
            "a target id carrying a character no resource name has, batched",
            "BatchGetDeploymentTargets",
            json!({"deploymentId": deployment_id, "targetIds": ["host a"]}),
            "InvalidDeploymentTargetIdException",
        ),
        (
            "a target id carrying a character no resource name has, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": "host/a"}),
            "InvalidDeploymentTargetIdException",
        ),
        (
            "a target id one character past the name bound, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": too_long}),
            "InvalidDeploymentTargetIdException",
        ),
        // At the bound the id is a NAME, so it is answered for what it names:
        // this deployment holds no target under it.
        (
            "a target id at the name bound, fetched",
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": at_the_bound}),
            "DeploymentTargetDoesNotExistException",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
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

/// RULE: the FORM of a target id is the one its deployment's compute platform
/// reports targets under — an Amazon ECS target is a `<clustername>:<servicename>`
/// pair, every other platform's is a resource name — and a value outside that
/// form names no target anywhere, which is `InvalidDeploymentTargetIdException`
/// rather than a target that does not exist.
#[tokio::test]
async fn a_target_id_outside_its_platforms_form_is_refused_as_malformed() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    group_with(&emulator, "server-app", "Server", &deployable()).await;
    let server = deploy(&emulator, "server-app").await;
    group_with(
        &emulator,
        "ecs-form-app",
        "ECS",
        &json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
    )
    .await;
    let ecs = deploy(&emulator, "ecs-form-app").await;
    terminal_deployment(&emulator, &server, "server form").await;
    terminal_deployment(&emulator, &ecs, "ecs form").await;

    // Rows differ along the platform the id is read against and along HOW the
    // id departs from that platform's form; `None` expects a 200, which only a
    // value that both has the form and names a resolved target gets.
    let cases: &[(&str, &str, &str, Option<&str>)] = &[
        ("an instance id", &server, "host-a", None),
        (
            "a pair, where a single name is the form",
            &server,
            "prod:web",
            Some("InvalidDeploymentTargetIdException"),
        ),
        (
            "a name carrying a space",
            &server,
            "host a",
            Some("InvalidDeploymentTargetIdException"),
        ),
        ("a cluster/service pair", &ecs, "prod:web", None),
        (
            "a single name, where a pair is the form",
            &ecs,
            "prodweb",
            Some("InvalidDeploymentTargetIdException"),
        ),
        (
            "a pair naming no cluster",
            &ecs,
            ":web",
            Some("InvalidDeploymentTargetIdException"),
        ),
        (
            "a pair naming no service",
            &ecs,
            "prod:",
            Some("InvalidDeploymentTargetIdException"),
        ),
        (
            "three names where a pair is the form",
            &ecs,
            "prod:web:extra",
            Some("InvalidDeploymentTargetIdException"),
        ),
        // A well-formed pair the deployment did not resolve: the form is right,
        // so it is answered for what it names.
        (
            "a well-formed pair of another service",
            &ecs,
            "prod:api",
            Some("DeploymentTargetDoesNotExistException"),
        ),
    ];

    for (case, deployment_id, target_id, expected) in cases {
        let (status, body) = emulator
            .call(
                "GetDeploymentTarget",
                json!({"deploymentId": deployment_id, "targetId": target_id}),
            )
            .await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the target to be answered, got {status} {body:?}"
            ),
            Some(expected) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *expected),
                "case {case:?}: expected a 400 {expected}, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a deployment of a compute platform whose deployment has exactly one
/// target is batch-read one target id at a time —
/// `DeploymentTargetListSizeExceededException` ("The target list of both types
/// of deployments must have exactly one item") — while an EC2/on-premises
/// deployment is bounded only by the 25 ids the operation publishes.
#[tokio::test]
async fn an_ecs_or_lambda_deployment_is_batch_read_one_target_at_a_time() {
    // Rows differ along the compute platform and along how many ids are named:
    // the two platforms the error names, and the one it explicitly excludes.
    struct Case<'a> {
        case: &'a str,
        application: &'a str,
        platform: &'a str,
        extras: Value,
        fleet: &'a [&'a str],
        /// The ids to batch, and the error code expected — `None` for an answer.
        attempts: &'a [(&'a [&'a str], Option<&'a str>)],
    }
    let cases: &[Case] = &[
        Case {
            case: "an Amazon ECS deployment of two services",
            application: "ecs-pair-app",
            platform: "ECS",
            extras: json!({"ecsServices": [
                {"clusterName": "prod", "serviceName": "web"},
                {"clusterName": "prod", "serviceName": "api"},
            ]}),
            fleet: &[],
            attempts: &[
                (&["prod:web"], None),
                (&["prod:api"], None),
                (
                    &["prod:web", "prod:api"],
                    Some("DeploymentTargetListSizeExceededException"),
                ),
            ],
        },
        Case {
            case: "a Lambda deployment, whose one target is named inside the revision",
            application: "lambda-app",
            platform: "Lambda",
            extras: json!({}),
            fleet: &[],
            attempts: &[
                // The function's name lives in the revision's AppSpec, which
                // CodeDeploy's state holds no content of, so no id reaches
                // this deployment's one target.
                (&["handler"], Some("DeploymentTargetDoesNotExistException")),
                (
                    &["handler", "other"],
                    Some("DeploymentTargetListSizeExceededException"),
                ),
            ],
        },
        Case {
            case: "an EC2/on-premises deployment, which the bound excludes",
            application: "server-pair-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-a", "host-b"],
            attempts: &[(&["host-a", "host-b"], None)],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for instance in case.fleet {
            register(&emulator, instance, &[("Fleet", "deployable")]).await;
        }
        group_with(&emulator, case.application, case.platform, &case.extras).await;
        let deployment_id = deploy(&emulator, case.application).await;
        terminal_deployment(&emulator, &deployment_id, case.case).await;

        for (target_ids, expected) in case.attempts {
            let (status, body) = emulator
                .call(
                    "BatchGetDeploymentTargets",
                    json!({"deploymentId": deployment_id, "targetIds": target_ids}),
                )
                .await;
            match expected {
                None => assert_eq!(
                    status,
                    200,
                    "case {:?} naming {target_ids:?}: expected the targets to be answered, got \
                     {status} {body:?}",
                    case.case
                ),
                Some(expected) => assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, *expected),
                    "case {:?} naming {target_ids:?}: expected a 400 {expected}, got {status} \
                     {body:?}",
                    case.case
                ),
            }
        }
    }
}

/// RULE: a reader does not change what a later caller sees — reading a target
/// leaves the deployment, its status and its targets exactly as they were.
#[tokio::test]
async fn reading_a_target_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    let application = "reread-app";
    group_with(&emulator, application, "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, application).await;
    terminal_deployment(&emulator, &deployment_id, "reread").await;

    let first = batch(&emulator, &deployment_id, &["host-a"]).await;
    let before = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    // Read the same target through both readers again, several times over.
    for _ in 0..3 {
        let again = batch(&emulator, &deployment_id, &["host-a"]).await;
        assert_eq!(
            again, first,
            "case {:?}: expected a re-read to answer the same target, got {again:?}",
            "re-read"
        );
        let fetched = get_one(&emulator, &deployment_id, "host-a").await;
        assert_eq!(
            fetched, first[0],
            "case {:?}: expected the individual read to answer the same target, got {fetched:?}",
            "re-read"
        );
    }
    let after = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    assert_eq!(
        after, before,
        "case {:?}: expected the deployment to be untouched by the reads, got {after:?}",
        "re-read"
    );
}
