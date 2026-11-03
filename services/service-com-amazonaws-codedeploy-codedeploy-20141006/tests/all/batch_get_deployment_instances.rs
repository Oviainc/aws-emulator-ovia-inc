// SPDX-License-Identifier: CC0-1.0

//! `BatchGetDeploymentInstances` over the wire.
//!
//! RULE: the call answers one `InstanceSummary` per instance id the request
//! names that the deployment's own record holds an instance under — the snapshot
//! its deployment group resolved when it was created — in the order the request
//! named them, each identical to what the single-instance reader
//! `GetDeploymentInstance` answers for that id; an id the deployment resolved no
//! instance under is simply absent from the answer, because this operation
//! models no does-not-exist error for its ids, while a request that cannot be
//! answered at all (an unanswerable deployment id, an id outside the form an
//! instance is reported under, a batch past the request-size quota, a deployment
//! on a compute platform that has no instances) is refused HTTP 400 with the
//! modelled error for that cause.
//!
//! The suites are TABLES: the rows span the deployment style (so the environment
//! an instance belongs to), how many of the registered instances the group's
//! selector takes in, the FORM of the instance names, and the SHAPE of the batch
//! asked for (every instance, a subset, reordered, repeated, mixed with ids the
//! deployment holds nothing under, at the request-size quota and one past it).
//!
//! The harness is the one [`super::codedeploy_lifecycle`] drives the rest of
//! this service with and the fleet helpers are
//! [`super::list_deployment_targets`]', so every case goes through the same
//! front door a client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, terminal_deployment, Emulator};
use crate::deployment_targets::deployable;
use crate::get_deployment_instance::{deployable_blue_green, summary_of};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// One deployment to read instances of: a group of `platform` carrying `extras`
/// over `fleet`, and the instance ids it resolves in resolution order.
struct Deployed<'a> {
    case: &'a str,
    application: &'a str,
    platform: &'a str,
    extras: Value,
    /// On-premises instances to register, as `(name, tags)`, in order.
    fleet: &'a [(&'a str, &'a [(&'a str, &'a str)])],
    resolved: &'a [&'a str],
}

/// One batch to ask for against a [`Deployed`] row: the ids requested and the
/// ids the answer must carry, in order.
struct Asked<'a> {
    case: &'a str,
    requested: &'a [&'a str],
    expected: &'a [&'a str],
}

/// Call the batch reader, answering its HTTP status and parsed body.
async fn batch(emulator: &Emulator, deployment_id: &str, instance_ids: &[String]) -> (u16, Value) {
    emulator
        .call(
            "BatchGetDeploymentInstances",
            json!({"deploymentId": deployment_id, "instanceIds": instance_ids}),
        )
        .await
}

/// The `instancesSummary` of a successful batch answer.
async fn summaries(emulator: &Emulator, deployment_id: &str, instance_ids: &[&str]) -> Vec<Value> {
    let owned: Vec<String> = instance_ids.iter().map(|id| (*id).to_string()).collect();
    let (status, body) = batch(emulator, deployment_id, &owned).await;
    assert_eq!(
        status, 200,
        "case {instance_ids:?}: expected the batch to be answered, got {status} {body:?}"
    );
    assert_eq!(
        body["errorMessage"], "",
        "case {instance_ids:?}: expected a batch that was answered to carry the no-error \
         errorMessage, got {body:?}"
    );
    body["instancesSummary"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `instanceId` of each entry of a batch answer, in answer order.
fn answered_ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["instanceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `instancesList` the deprecated listing answers for the same deployment.
async fn listed_instances(emulator: &Emulator, deployment_id: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeploymentInstances",
            json!({"deploymentId": deployment_id}),
        )
        .await["instancesList"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Register a row's fleet, create its group and deploy to it, answering the
/// deployment id.
async fn seed(emulator: &Emulator, row: &Deployed<'_>) -> String {
    for (instance, tags) in row.fleet {
        register(emulator, instance, tags).await;
    }
    group_with(emulator, row.application, row.platform, &row.extras).await;
    deploy(emulator, row.application).await
}

/// A fleet of which the group's selector takes in the first two.
const MIXED_FLEET: &[(&str, &[(&str, &str)])] = &[
    ("host-a", &[("Fleet", "deployable")]),
    ("host-b", &[("Fleet", "deployable")]),
    ("host-c", &[("Fleet", "retired")]),
];

/// Names spanning the service's resource-name character set
/// (`smithy_query(query="shape",shape="ApplicationName")`: pattern
/// `^[A-Za-z0-9+=,.@_-]*$`), none of them the one the issue cited.
const VARIED_FLEET: &[(&str, &[(&str, &str)])] = &[
    ("web.01", &[("Fleet", "deployable")]),
    ("HOST_B", &[("Fleet", "deployable")]),
    ("node+1=2,3@eu", &[("Fleet", "deployable")]),
];

/// A fleet of one instance, which the group selects.
const SINGLE_FLEET: &[(&str, &[(&str, &str)])] = &[("solo", &[("Fleet", "deployable")])];

// ---------------------------------------------------------------------------
// What the batch answers
// ---------------------------------------------------------------------------

/// RULE: the batch answers the requested ids the deployment holds an instance
/// under, in request order, each exactly as the single-instance reader answers
/// it; an id it holds no instance under is left out rather than refusing the
/// batch.
#[tokio::test]
async fn the_batch_answers_each_requested_instance_as_the_single_reader_does() {
    let rows: &[(Deployed, &[Asked])] = &[
        (
            Deployed {
                case: "an in-place deployment to two of three registered instances",
                application: "batch-in-place-app",
                platform: "Server",
                extras: deployable(),
                fleet: MIXED_FLEET,
                resolved: &["host-a", "host-b"],
            },
            &[
                Asked {
                    case: "every instance, in resolution order",
                    requested: &["host-a", "host-b"],
                    expected: &["host-a", "host-b"],
                },
                Asked {
                    case: "every instance, in the reverse of resolution order",
                    requested: &["host-b", "host-a"],
                    expected: &["host-b", "host-a"],
                },
                Asked {
                    case: "one instance of the two",
                    requested: &["host-b"],
                    expected: &["host-b"],
                },
                Asked {
                    case: "one instance named twice",
                    requested: &["host-a", "host-a"],
                    expected: &["host-a", "host-a"],
                },
                Asked {
                    case: "a registered instance the group's selector left out",
                    requested: &["host-a", "host-c"],
                    expected: &["host-a"],
                },
                Asked {
                    case: "only ids the deployment holds no instance under",
                    requested: &["host-z", "host-y"],
                    expected: &[],
                },
            ],
        ),
        (
            Deployed {
                case: "a blue/green deployment to two of three registered instances",
                application: "batch-blue-green-app",
                platform: "Server",
                extras: deployable_blue_green(),
                fleet: MIXED_FLEET,
                resolved: &["host-a", "host-b"],
            },
            &[
                Asked {
                    case: "every instance of the original environment",
                    requested: &["host-a", "host-b"],
                    expected: &["host-a", "host-b"],
                },
                Asked {
                    case: "an instance of the deployment either side of an id it has none under",
                    requested: &["host-a", "host-z", "host-b"],
                    expected: &["host-a", "host-b"],
                },
            ],
        ),
        (
            Deployed {
                case: "an in-place deployment to instances whose names span the name set",
                application: "batch-varied-names-app",
                platform: "Server",
                extras: deployable(),
                fleet: VARIED_FLEET,
                resolved: &["web.01", "HOST_B", "node+1=2,3@eu"],
            },
            &[
                Asked {
                    case: "every instance, in resolution order",
                    requested: &["web.01", "HOST_B", "node+1=2,3@eu"],
                    expected: &["web.01", "HOST_B", "node+1=2,3@eu"],
                },
                Asked {
                    case: "the middle instance alone",
                    requested: &["HOST_B"],
                    expected: &["HOST_B"],
                },
                Asked {
                    case: "the instances out of resolution order, around an unknown id",
                    requested: &["node+1=2,3@eu", "nothing.here", "web.01"],
                    expected: &["node+1=2,3@eu", "web.01"],
                },
            ],
        ),
        (
            Deployed {
                case: "a blue/green deployment to one instance",
                application: "batch-single-instance-app",
                platform: "Server",
                extras: deployable_blue_green(),
                fleet: SINGLE_FLEET,
                resolved: &["solo"],
            },
            &[
                Asked {
                    case: "the one instance",
                    requested: &["solo"],
                    expected: &["solo"],
                },
                Asked {
                    case: "the one instance beside an id of another fleet",
                    requested: &["duo", "solo"],
                    expected: &["solo"],
                },
            ],
        ),
    ];

    for (row, batches) in rows {
        let emulator = Emulator::new();
        let deployment_id = seed(&emulator, row).await;
        // The deployment is held mid-flight so the two readers are compared on
        // one state of one record, whatever the machine's own timing: what the
        // clock owes the record is the next case's subject.
        force_status(&emulator, &deployment_id, "InProgress", true);
        assert_eq!(
            listed_instances(&emulator, &deployment_id).await,
            row.resolved,
            "case {:?}: expected the deployment to have resolved {:?}",
            row.case,
            row.resolved
        );

        for asked in *batches {
            let case = format!("{} asked for {}", row.case, asked.case);
            let answered = summaries(&emulator, &deployment_id, asked.requested).await;
            assert_eq!(
                answered_ids(&answered),
                asked.expected,
                "case {case:?}: expected the instances {:?}, got {answered:?}",
                asked.expected
            );
            for (position, instance_id) in asked.expected.iter().enumerate() {
                let single = summary_of(&emulator, &deployment_id, instance_id).await;
                assert_eq!(
                    answered[position], single,
                    "case {case:?}: expected entry {position} to be what the single reader \
                     answers for {instance_id} ({single:?}), got {:?}",
                    answered[position]
                );
                assert_eq!(
                    answered[position]["deploymentId"].as_str(),
                    Some(deployment_id.as_str()),
                    "case {case:?}: expected entry {position} to name its deployment, got {:?}",
                    answered[position]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The status the batch reports
// ---------------------------------------------------------------------------

/// RULE: every answered instance carries the `InstanceStatus` its deployment's
/// own progress implies, the one the single reader answers — and a deployment
/// the clock has not started yet is REPORTED, with its instances pending, rather
/// than refused: this operation models no not-started error.
#[tokio::test]
async fn the_batch_reports_the_status_the_deployments_progress_implies() {
    let emulator = Emulator::new();
    for (instance, tags) in MIXED_FLEET {
        register(&emulator, instance, tags).await;
    }
    group_with(&emulator, "batch-status-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "batch-status-app").await;
    let requested: &[&str] = &["host-a", "host-b"];

    // Rows span the model's whole `DeploymentStatus` enum
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued
    // | InProgress | Baking | Succeeded | Failed | Stopped | Ready), including
    // the statuses this emulator reaches by no call of its own, and both sides
    // of whether the deployment has started: `(case, deployment status,
    // started, expected instance status)`.
    let cases: &[(&str, &str, bool, &str)] = &[
        ("a deployment the clock has not started", "Created", false, "Pending"),
        ("a queued deployment", "Queued", false, "Pending"),
        ("a deployment under way", "InProgress", true, "InProgress"),
        ("a deployment baking", "Baking", true, "InProgress"),
        ("a deployment ready for traffic", "Ready", true, "InProgress"),
        ("a deployment that succeeded", "Succeeded", true, "Succeeded"),
        ("a deployment that failed", "Failed", true, "Failed"),
        // A caller stopped the deployment, so the revision was never finished
        // on its instances: skipped, not failed.
        ("a deployment a caller stopped", "Stopped", true, "Skipped"),
    ];

    for (case, deployment_status, started, expected) in cases {
        force_status(&emulator, &deployment_id, deployment_status, *started);
        let answered = summaries(&emulator, &deployment_id, requested).await;
        assert_eq!(
            answered_ids(&answered),
            requested,
            "case {case:?}: expected both instances to be answered, got {answered:?}"
        );
        assert!(
            codedeploy::ops::types::INSTANCE_STATUS_VALUES.contains(expected),
            "case {case:?}: expected {expected} to be a modelled InstanceStatus"
        );
        for summary in &answered {
            assert_eq!(
                summary["status"].as_str(),
                Some(*expected),
                "case {case:?}: expected the instance status {expected}, got {summary:?}"
            );
            assert!(
                summary["lastUpdatedAt"].as_f64().is_some(),
                "case {case:?}: expected the instant the instance last moved, got {summary:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The request-size quota
// ---------------------------------------------------------------------------

/// RULE: a batch of at most the service's request-size quota of ids is answered,
/// and one id past it is refused with `BatchLimitExceededException` — whatever
/// the ids in it are, and without reading the store.
#[tokio::test]
async fn a_batch_is_accepted_up_to_the_request_size_quota_and_refused_past_it() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    group_with(&emulator, "batch-quota-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "batch-quota-app").await;
    force_status(&emulator, &deployment_id, "InProgress", true);

    // The quota is the one `BatchLimitExceededException` states for every batch
    // reader of this service (`state::MAX_BATCH_SIZE`): the rows are one below
    // it, exactly it, and one past it.
    let quota = codedeploy::state::MAX_BATCH_SIZE;
    let padding = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("filler-{index}")).collect()
    };
    let cases: &[(&str, usize, Option<&str>)] = &[
        ("one id below the quota", quota - 1, None),
        ("exactly the quota", quota, None),
        ("one id past the quota", quota + 1, Some("BatchLimitExceededException")),
    ];

    for (case, count, expected) in cases {
        // The one instance the deployment holds, followed by well-formed ids it
        // holds nothing under, so the row differs only in HOW MANY ids it sends.
        let mut ids = vec!["host-a".to_string()];
        ids.extend(padding(count - 1));
        assert_eq!(ids.len(), *count, "case {case:?}: the row sends {count} ids");

        let (status, body) = batch(&emulator, &deployment_id, &ids).await;
        match expected {
            Some(code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, *code),
                    "case {case:?}: expected a 400 {code}, got {status} {body:?}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(codedeploy::state::batch_limit_exceeded_message().as_str()),
                    "case {case:?}: expected the refusal to name the quota, got {body:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the batch to be answered, got {status} {body:?}"
                );
                let answered = body["instancesSummary"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    answered_ids(&answered),
                    ["host-a"],
                    "case {case:?}: expected only the id the deployment holds an instance under, \
                     got {answered:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request this operation cannot answer is refused with the modelled
/// error the model gives it for that cause, and HTTP 400 — the deployment id's
/// FORM before the lookup, and whether the deployment reports instances at all
/// before which of them it holds.
#[tokio::test]
async fn a_request_that_cannot_be_answered_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;

    group_with(&emulator, "batch-refusal-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "batch-refusal-app").await;
    terminal_deployment(&emulator, &deployment_id, "batch refusals").await;

    group_with(
        &emulator,
        "batch-refusal-ecs-app",
        "ECS",
        &json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
    )
    .await;
    let ecs_deployment_id = deploy(&emulator, "batch-refusal-ecs-app").await;
    terminal_deployment(&emulator, &ecs_deployment_id, "batch refusals").await;

    // A name one character past the longest the service accepts
    // (`state::MAX_NAME_LENGTH`).
    let too_long = "n".repeat(codedeploy::state::MAX_NAME_LENGTH + 1);
    // Rows differ along WHICH part of the request is unanswerable: the
    // deployment id (absent, malformed, or naming no deployment), the instance
    // ids (an absent list, an empty entry, an entry outside the name set), and
    // the compute platform the deployment is on.
    let cases: &[(&str, Value, &str)] = &[
        (
            "an empty deployment id",
            json!({"deploymentId": "", "instanceIds": ["host-a"]}),
            "DeploymentIdRequiredException",
        ),
        (
            "a blank deployment id",
            json!({"deploymentId": "   ", "instanceIds": ["host-a"]}),
            "DeploymentIdRequiredException",
        ),
        (
            "a deployment id of no recognisable form",
            json!({"deploymentId": "not-an-id", "instanceIds": ["host-a"]}),
            "InvalidDeploymentIdException",
        ),
        (
            "a deployment id whose suffix is lowercase",
            json!({"deploymentId": "d-abcdefghi", "instanceIds": ["host-a"]}),
            "InvalidDeploymentIdException",
        ),
        (
            "a well-formed deployment id no deployment has",
            json!({"deploymentId": "d-ZZZZZZZZZ", "instanceIds": ["host-a"]}),
            "DeploymentDoesNotExistException",
        ),
        (
            "an empty list of instance ids",
            json!({"deploymentId": deployment_id, "instanceIds": []}),
            "InstanceIdRequiredException",
        ),
        (
            "an empty instance id in the batch",
            json!({"deploymentId": deployment_id, "instanceIds": ["host-a", ""]}),
            "InstanceIdRequiredException",
        ),
        (
            "a blank instance id in the batch",
            json!({"deploymentId": deployment_id, "instanceIds": ["  "]}),
            "InstanceIdRequiredException",
        ),
        (
            "an instance id outside the service's name set",
            json!({"deploymentId": deployment_id, "instanceIds": ["host-a", "prod:web"]}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance id one character too long",
            json!({"deploymentId": deployment_id, "instanceIds": [too_long]}),
            "InvalidInstanceNameException",
        ),
        // The platform the model's own sentence excludes: an Amazon ECS
        // deployment is a perfectly good deployment that simply has no
        // instances.
        (
            "an Amazon ECS deployment, which has no instances",
            json!({"deploymentId": ecs_deployment_id, "instanceIds": ["host-a"]}),
            "InvalidComputePlatformException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("BatchGetDeploymentInstances", request.clone())
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

    // A Lambda deployment DOES report instances, but its one target is the
    // function its revision's AppSpec names, which CodeDeploy's state holds no
    // content of — so no id reaches it, and the batch is ANSWERED empty rather
    // than refused.
    group_with(&emulator, "batch-refusal-lambda-app", "Lambda", &json!({})).await;
    let lambda_deployment_id = deploy(&emulator, "batch-refusal-lambda-app").await;
    terminal_deployment(&emulator, &lambda_deployment_id, "batch refusals").await;
    let answered = summaries(&emulator, &lambda_deployment_id, &["orderProcessor"]).await;
    assert!(
        answered.is_empty(),
        "case \"a Lambda deployment, whose one instance is named inside the revision\": expected \
         an empty answer, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading a batch of instances reports the stored records and moves
/// nothing — two identical batches answer the same, and what every other reader
/// of that deployment answers is unchanged by the read.
#[tokio::test]
async fn reading_a_batch_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    for (instance, tags) in MIXED_FLEET {
        register(&emulator, instance, tags).await;
    }
    group_with(&emulator, "batch-stable-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "batch-stable-app").await;
    // Read on a deployment the clock has nothing left to owe, so a difference
    // between two reads can only be this operation's doing.
    let before = terminal_deployment(&emulator, &deployment_id, "stable batch reads").await;
    let listed_before = listed_instances(&emulator, &deployment_id).await;

    let requested: &[&str] = &["host-b", "host-a"];
    let first = summaries(&emulator, &deployment_id, requested).await;
    // Reading one batch must not move what another answers, so a second batch
    // over one of the same instances is read in between.
    let narrower = summaries(&emulator, &deployment_id, &["host-a"]).await;
    let second = summaries(&emulator, &deployment_id, requested).await;
    assert_eq!(
        first, second,
        "case \"two identical batches\": expected the same answer, got {first:?} then {second:?}"
    );
    assert_eq!(
        narrower.first().and_then(|summary| summary["status"].as_str()),
        first.first().and_then(|summary| summary["status"].as_str()),
        "case \"a narrower batch of the same deployment\": expected the same status as {first:?}, \
         got {narrower:?}"
    );

    let after = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone();
    assert_eq!(
        after["status"].as_str(),
        before["status"].as_str(),
        "case \"the deployment behind the instances\": expected the read to leave its status at \
         {:?}, got {after:?}",
        before["status"]
    );
    assert_eq!(
        listed_instances(&emulator, &deployment_id).await,
        listed_before,
        "case \"the listing behind the instances\": expected the read to leave it at \
         {listed_before:?}"
    );
}
