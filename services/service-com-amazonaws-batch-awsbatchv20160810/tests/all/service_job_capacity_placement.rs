// SPDX-License-Identifier: CC0-1.0

//! Whether a service job can be PLACED, end to end over the wire.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a service job is placed only when the capacity its request asks for fits the capacity
//!   limits the service environments of its queue publish; a job that fits is walked on by
//!   the clock, and one that does NOT fit is never placed, so it RESTS in the non-terminal
//!   `RUNNABLE` status instead of reaching any terminal status — no `statusReason`,
//!   `stoppedAt` or attempt is reported for work the scheduler never handed on;
//! * a job resting in `RUNNABLE` stays readable: `DescribeServiceJob` reads every member
//!   the submit carried back unchanged — the integers as submitted, the payload
//!   byte-identical — and reports an optional member the submit never carried as ABSENT;
//! * a read never moves a resting job, and the capacity a later call publishes — a bigger
//!   `capacityLimits` on the environment (`UpdateServiceEnvironment`) or a
//!   `serviceEnvironmentOrder` naming an environment that can hold it (`UpdateJobQueue`) —
//!   is what makes its next move due;
//! * every caller-filled list this rule reads is kept to a maximum — the payload's bytes,
//!   an environment's `capacityLimits` and a queue's `serviceEnvironmentOrder` — accepted
//!   on the maximum and refused one past it with the modelled `ClientException`.
//!
//! The capacity a `SAGEMAKER_TRAINING` request asks for is its `ResourceConfig`
//! (`InstanceType`/`InstanceCount`, or one `InstanceGroups` entry per group), and a limit
//! is measured either in an instance type or in `NUM_INSTANCES` —
//! `CapacityLimit$capacityUnit`: "For `SAGEMAKER_TRAINING` jobs in a quota management
//! enabled service environment, specify the instance type (for example, `ml.m5.large`).
//! Otherwise, use `NUM_INSTANCES`."

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{error_code, error_message, Batch};

/// The only `ServiceJobType`/`ServiceEnvironmentType` the model declares
/// (`smithy_query(query="shape", shape="ServiceJobType")`).
const SAGEMAKER_TRAINING: &str = "SAGEMAKER_TRAINING";

/// An instance type a `ResourceConfig` can ask for, and a `capacityUnit` can be measured
/// in: the example the `capacityUnit` doc itself names.
const SMALL_INSTANCE_TYPE: &str = "ml.m5.large";

/// A second instance type, so a limit published for one type can be shown NOT to admit
/// another. Named in the `ResourceConfig` doc's instance-type list.
const LARGE_INSTANCE_TYPE: &str = "ml.p4d.24xlarge";

/// Long enough for every clock-owed service-job transition to have fallen due: a placeable
/// job makes two ([`state::JOB_PHASE_MILLIS`] each), and the margin covers a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 4)).await;
}

/// A service environment publishing `capacity_limits`, and a `SAGEMAKER_TRAINING` queue
/// over it; answers the queue's name.
async fn environment_and_queue(batch: &Batch, suffix: &str, capacity_limits: Value) -> String {
    let environment = format!("se-{suffix}");
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": &environment,
                "serviceEnvironmentType": SAGEMAKER_TRAINING,
                "capacityLimits": capacity_limits,
            }),
        )
        .await;
    let queue = format!("jq-{suffix}");
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": &queue,
                "priority": 1,
                "jobQueueType": SAGEMAKER_TRAINING,
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": &environment}],
            }),
        )
        .await;
    queue
}

/// `SubmitServiceJob` on `queue` with `payload`, answering the minted job id.
async fn submit(batch: &Batch, suffix: &str, queue: &str, payload: &str) -> String {
    let accepted = batch
        .ok(
            "/v1/submitservicejob",
            json!({
                "jobName": format!("sj-{suffix}"),
                "jobQueue": queue,
                "serviceJobType": SAGEMAKER_TRAINING,
                "serviceRequestPayload": payload,
            }),
        )
        .await;
    accepted["jobId"].as_str().unwrap_or_default().to_string()
}

/// `DescribeServiceJob`, requiring the 200.
async fn described(batch: &Batch, case: &str, job_id: &str) -> Value {
    let (status, body) = batch
        .call("/v1/describeservicejob", json!({"jobId": job_id}))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeServiceJob to answer 200, got {status} {body}"
    );
    body
}

/// One placement row: the capacity an environment publishes, the capacity a submit asks
/// for, and whether the job can therefore be placed.
struct Placement {
    /// What this row varies; named in every assertion.
    label: &'static str,
    /// The suffix the row's environment, queue and job are named with.
    suffix: &'static str,
    capacity_limits: Value,
    payload: String,
    /// Whether the published capacity admits the request.
    placeable: bool,
}

/// The table: rows differ along what the rule generalises over — the unit a limit is
/// measured in (`NUM_INSTANCES` or an instance type), the maximum it publishes, how many
/// instances of which type the request asks for, whether the request states ONE
/// `ResourceConfig` or several `InstanceGroups`, and whether it states any capacity at all.
/// Each published maximum is also tested ON the bound and one past it.
fn placements() -> Vec<Placement> {
    vec![
        Placement {
            label: "two instances against a NUM_INSTANCES maximum of one",
            suffix: "over-num",
            capacity_limits: json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
            payload: json!({
                "ResourceConfig": {
                    "InstanceType": SMALL_INSTANCE_TYPE,
                    "InstanceCount": 2,
                    "VolumeSizeInGB": 13,
                },
                "StoppingCondition": {"MaxRuntimeInSeconds": 45},
            })
            .to_string(),
            placeable: false,
        },
        Placement {
            label: "one instance against a NUM_INSTANCES maximum of one (on the bound)",
            suffix: "on-num",
            capacity_limits: json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
            payload: json!({
                "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 1},
            })
            .to_string(),
            placeable: true,
        },
        Placement {
            label: "two instance groups summing past a NUM_INSTANCES maximum of four",
            suffix: "over-groups",
            capacity_limits: json!([{"maxCapacity": 4, "capacityUnit": "NUM_INSTANCES"}]),
            payload: json!({
                "ResourceConfig": {
                    "InstanceGroups": [
                        {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 2},
                        {"InstanceType": LARGE_INSTANCE_TYPE, "InstanceCount": 3},
                    ],
                },
            })
            .to_string(),
            placeable: false,
        },
        Placement {
            label: "two instance groups summing exactly to a NUM_INSTANCES maximum of four",
            suffix: "on-groups",
            capacity_limits: json!([{"maxCapacity": 4, "capacityUnit": "NUM_INSTANCES"}]),
            payload: json!({
                "ResourceConfig": {
                    "InstanceGroups": [
                        {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 2},
                        {"InstanceType": LARGE_INSTANCE_TYPE, "InstanceCount": 2},
                    ],
                },
            })
            .to_string(),
            placeable: true,
        },
        Placement {
            label: "two instances of the type a limit of two is measured in",
            suffix: "on-type",
            capacity_limits: json!([{"maxCapacity": 2, "capacityUnit": SMALL_INSTANCE_TYPE}]),
            payload: json!({
                "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 2},
            })
            .to_string(),
            placeable: true,
        },
        Placement {
            label: "three instances of the type a limit of two is measured in",
            suffix: "over-type",
            capacity_limits: json!([{"maxCapacity": 2, "capacityUnit": SMALL_INSTANCE_TYPE}]),
            payload: json!({
                "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 3},
            })
            .to_string(),
            placeable: false,
        },
        Placement {
            label: "an instance type no published limit is measured in",
            suffix: "other-type",
            capacity_limits: json!([{"maxCapacity": 8, "capacityUnit": SMALL_INSTANCE_TYPE}]),
            payload: json!({
                "ResourceConfig": {"InstanceType": LARGE_INSTANCE_TYPE, "InstanceCount": 1},
            })
            .to_string(),
            placeable: false,
        },
        Placement {
            label: "a request that states no ResourceConfig at all",
            suffix: "no-config",
            capacity_limits: json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
            payload: json!({"TrainingJobName": "t-no-config"}).to_string(),
            placeable: true,
        },
        Placement {
            label: "an environment that publishes no capacity limits",
            suffix: "no-limits",
            capacity_limits: json!([]),
            payload: json!({
                "ResourceConfig": {"InstanceType": LARGE_INSTANCE_TYPE, "InstanceCount": 64},
            })
            .to_string(),
            placeable: true,
        },
    ]
}

/// RULE: a service job is placed only when the capacity its request asks for fits what the
/// service environments of its queue publish; one that does not fit rests in the
/// non-terminal `RUNNABLE` status with nothing terminal invented for it, while one that
/// fits is walked on by the clock.
#[tokio::test]
async fn a_service_job_is_placed_only_within_the_capacity_its_environment_publishes() {
    let batch = Batch::new();

    // Every row is submitted first, so ONE wait covers the transitions all of them owe.
    let mut submitted: Vec<(Placement, String)> = Vec::new();
    for row in placements() {
        let queue =
            environment_and_queue(&batch, row.suffix, row.capacity_limits.clone()).await;
        let job_id = submit(&batch, row.suffix, &queue, &row.payload).await;
        submitted.push((row, job_id));
    }
    wait_for_the_clock().await;

    for (row, job_id) in &submitted {
        let case = row.label;
        let detail = described(&batch, case, job_id).await;
        if row.placeable {
            assert_eq!(
                detail["status"].as_str(),
                Some(state::SERVICE_JOB_FAILED),
                "case {case}: a placeable job is walked on by the clock, and nothing here \
                 runs SageMaker AI, got {detail}"
            );
            assert_eq!(
                detail["statusReason"].as_str(),
                Some(state::NO_SERVICE_JOB_RUNTIME_REASON),
                "case {case}: the failure must say why nothing ran, got {detail}"
            );
            continue;
        }
        assert_eq!(
            detail["status"].as_str(),
            Some(state::SERVICE_JOB_RUNNABLE),
            "case {case}: a job the published capacity cannot hold rests in RUNNABLE, \
             got {detail}"
        );
        // Nothing placed or dispatched the job, so nothing a placement would produce is
        // reported for it.
        for member in [
            "statusReason",
            "stoppedAt",
            "scheduledAt",
            "attempts",
            "latestAttempt",
            "capacityUsage",
            "preemptionSummary",
        ] {
            assert_eq!(
                detail[member], Value::Null,
                "case {case}: a resting job reports no {member}, got {detail}"
            );
        }
        assert_eq!(
            detail["isTerminated"].as_bool(),
            Some(false),
            "case {case}: nothing terminated a resting job, got {detail}"
        );

        // A read never moves it.
        let again = described(&batch, case, job_id).await;
        assert_eq!(
            again, detail,
            "case {case}: two reads at the same instant answer identically, got {again}"
        );
    }

    // Nor does more time passing: ONE further wait covers every resting row.
    wait_for_the_clock().await;
    for (row, job_id) in &submitted {
        if row.placeable {
            continue;
        }
        let case = row.label;
        let later = described(&batch, case, job_id).await;
        assert_eq!(
            later["status"].as_str(),
            Some(state::SERVICE_JOB_RUNNABLE),
            "case {case}: a job that still cannot be placed still rests in RUNNABLE, \
             got {later}"
        );
    }
}

/// RULE: a service job resting in `RUNNABLE` reads back every member the submit carried,
/// unchanged — the integers as submitted and the payload byte-identical — and an optional
/// member the submit never carried is ABSENT rather than defaulted.
#[tokio::test]
async fn a_resting_service_job_reads_every_submitted_member_back_unchanged() {
    let batch = Batch::new();
    let queue = environment_and_queue(
        &batch,
        "rest-roundtrip",
        json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
    )
    .await;

    // The rows differ along the values the rule must preserve: the priorities and attempt
    // counts on the bounds their own docs publish, the timeout in between, and payloads
    // whose embedded JSON numbers must keep their precision (a long integer, a decimal and
    // an exponent).
    let rows: Vec<(&str, i64, i64, i64, &str, String)> = vec![
        (
            "the recorded shape: priority 17, one attempt, 999 seconds",
            17,
            1,
            999,
            "default",
            r#"{"ResourceConfig":{"InstanceType":"ml.m5.large","InstanceCount":2,"VolumeSizeInGB":13},"StoppingCondition":{"MaxRuntimeInSeconds":45}}"#
                .to_string(),
        ),
        (
            "both members on their published minima, a decimal in the payload",
            i64::from(state::MIN_SCHEDULING_PRIORITY),
            i64::from(state::MIN_SERVICE_JOB_RETRY_ATTEMPTS),
            1,
            "s",
            r#"{"ResourceConfig":{"InstanceCount":9,"KeepAlivePeriodInSeconds":1800},"HyperParameters":{"lr":0.000012345678901234567}}"#
                .to_string(),
        ),
        (
            "both members on their published maxima, a long integer and an exponent",
            i64::from(state::MAX_SCHEDULING_PRIORITY),
            i64::from(state::MAX_SERVICE_JOB_RETRY_ATTEMPTS),
            86400,
            "shareWithALongerName",
            r#"{"ResourceConfig":{"InstanceCount":3,"VolumeSizeInGB":12345678901234567890},"StoppingCondition":{"MaxRuntimeInSeconds":1.0e3}}"#
                .to_string(),
        ),
    ];

    // Every row is submitted first, so ONE wait covers the transitions all of them owe.
    let mut submitted: Vec<String> = Vec::new();
    for (index, (_, priority, attempts, seconds, share, payload)) in rows.iter().enumerate() {
        let accepted = batch
            .ok(
                "/v1/submitservicejob",
                json!({
                    "jobName": format!("sj-rest-{index}"),
                    "jobQueue": &queue,
                    "serviceJobType": SAGEMAKER_TRAINING,
                    "serviceRequestPayload": payload,
                    "schedulingPriority": priority,
                    "shareIdentifier": share,
                    "timeoutConfig": {"attemptDurationSeconds": seconds},
                    "retryStrategy": {"attempts": attempts},
                }),
            )
            .await;
        submitted.push(accepted["jobId"].as_str().unwrap_or_default().to_string());
    }
    wait_for_the_clock().await;

    for ((case, priority, attempts, seconds, share, payload), job_id) in
        rows.iter().zip(&submitted)
    {
        let detail = described(&batch, case, job_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(state::SERVICE_JOB_RUNNABLE),
            "case {case}: the request asks for more instances than the environment \
             publishes, so the job rests in RUNNABLE, got {detail}"
        );
        assert_eq!(
            detail["schedulingPriority"].as_i64(),
            Some(*priority),
            "case {case}: expected the submitted schedulingPriority, got {detail}"
        );
        assert_eq!(
            detail["retryStrategy"]["attempts"].as_i64(),
            Some(*attempts),
            "case {case}: expected the submitted retryStrategy.attempts, got {detail}"
        );
        assert_eq!(
            detail["timeoutConfig"]["attemptDurationSeconds"].as_i64(),
            Some(*seconds),
            "case {case}: expected the submitted attemptDurationSeconds, got {detail}"
        );
        assert_eq!(
            detail["shareIdentifier"].as_str(),
            Some(*share),
            "case {case}: expected the submitted shareIdentifier, got {detail}"
        );
        assert_eq!(
            detail["serviceRequestPayload"].as_str(),
            Some(payload.as_str()),
            "case {case}: the payload is persisted byte-identical, got {detail}"
        );
        assert_eq!(
            detail["quotaShareName"], Value::Null,
            "case {case}: a member the submit never carried stays absent, got {detail}"
        );
    }
}

/// RULE: the capacity a later call publishes is what moves a resting job on — raising the
/// environment's `capacityLimits` makes the next move of a job that now fits due, while an
/// `UpdateServiceEnvironment` that publishes capacity still too small leaves it resting.
#[tokio::test]
async fn capacity_published_later_is_what_moves_a_resting_service_job_on() {
    let batch = Batch::new();
    let environment = "se-rearm";
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": environment,
                "serviceEnvironmentType": SAGEMAKER_TRAINING,
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-rearm",
                "priority": 1,
                "jobQueueType": SAGEMAKER_TRAINING,
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": environment}],
            }),
        )
        .await;
    let payload = json!({
        "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 4},
    })
    .to_string();
    let job_id = submit(&batch, "rearm", "jq-rearm", &payload).await;

    wait_for_the_clock().await;
    let resting = described(&batch, "before the capacity grows", &job_id).await;
    assert_eq!(
        resting["status"].as_str(),
        Some(state::SERVICE_JOB_RUNNABLE),
        "case before the capacity grows: expected the job to rest in RUNNABLE, got {resting}"
    );

    // Still too small: the job owes nothing, so it stays where it is.
    batch
        .ok(
            "/v1/updateserviceenvironment",
            json!({
                "serviceEnvironment": environment,
                "capacityLimits": [{"maxCapacity": 3, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    wait_for_the_clock().await;
    let still_resting = described(&batch, "capacity still too small", &job_id).await;
    assert_eq!(
        still_resting["status"].as_str(),
        Some(state::SERVICE_JOB_RUNNABLE),
        "case capacity still too small: expected the job to keep resting, got {still_resting}"
    );

    // Enough capacity: the move is owed again, and a span later the clock has made it.
    batch
        .ok(
            "/v1/updateserviceenvironment",
            json!({
                "serviceEnvironment": environment,
                "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    let straight_after = described(&batch, "straight after the capacity grows", &job_id).await;
    assert_eq!(
        straight_after["status"].as_str(),
        Some(state::SERVICE_JOB_RUNNABLE),
        "case straight after the capacity grows: the owed move takes a span, so the \
         transient status is still observable, got {straight_after}"
    );
    wait_for_the_clock().await;
    let moved = described(&batch, "after the capacity grows", &job_id).await;
    assert_eq!(
        moved["status"].as_str(),
        Some(state::SERVICE_JOB_FAILED),
        "case after the capacity grows: the placed job is walked on by the clock, and \
         nothing here runs SageMaker AI, got {moved}"
    );
    assert_eq!(
        moved["statusReason"].as_str(),
        Some(state::NO_SERVICE_JOB_RUNTIME_REASON),
        "case after the capacity grows: the failure must say why nothing ran, got {moved}"
    );
}

/// RULE: the same holds for the OTHER call that changes which capacity a job's queue can
/// reach — an `UpdateJobQueue` that points the queue's `serviceEnvironmentOrder` at an
/// environment publishing enough capacity makes the resting job's next move due.
#[tokio::test]
async fn pointing_a_queue_at_a_bigger_environment_moves_a_resting_service_job_on() {
    let batch = Batch::new();
    let queue = environment_and_queue(
        &batch,
        "reorder-small",
        json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
    )
    .await;
    // A second environment, publishing room for the request the small one cannot hold.
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-reorder-large",
                "serviceEnvironmentType": SAGEMAKER_TRAINING,
                "capacityLimits": [{"maxCapacity": 16, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    let payload = json!({
        "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 6},
    })
    .to_string();
    let job_id = submit(&batch, "reorder", &queue, &payload).await;

    wait_for_the_clock().await;
    let resting = described(&batch, "before the queue is re-pointed", &job_id).await;
    assert_eq!(
        resting["status"].as_str(),
        Some(state::SERVICE_JOB_RUNNABLE),
        "case before the queue is re-pointed: expected the job to rest in RUNNABLE, \
         got {resting}"
    );

    batch
        .ok(
            "/v1/updatejobqueue",
            json!({
                "jobQueue": &queue,
                "serviceEnvironmentOrder": [
                    {"order": 1, "serviceEnvironment": "se-reorder-large"},
                ],
            }),
        )
        .await;
    wait_for_the_clock().await;
    let moved = described(&batch, "after the queue is re-pointed", &job_id).await;
    assert_eq!(
        moved["status"].as_str(),
        Some(state::SERVICE_JOB_FAILED),
        "case after the queue is re-pointed: the job the new environment can hold is \
         walked on by the clock, and nothing here runs SageMaker AI, got {moved}"
    );
}

/// RULE: every caller-filled list and payload this placement rule reads is kept to a
/// maximum, so one request cannot decide how much work a later one costs: a
/// `serviceRequestPayload` past `MAX_SERVICE_REQUEST_PAYLOAD_BYTES`, a `capacityLimits`
/// past `MAX_SERVICE_ENVIRONMENT_CAPACITY_LIMITS` and a `serviceEnvironmentOrder` past
/// `MAX_SERVICE_ENVIRONMENT_ORDER_ENTRIES` are each the modelled `ClientException` (400),
/// while a request ON each maximum is accepted.
#[tokio::test]
async fn the_lists_placement_reads_are_each_kept_to_their_maximum() {
    let batch = Batch::new();

    // ON the maximum: as many capacity limits as an environment keeps, each measured in a
    // distinct unit, and a queue over it with as many order entries as a queue keeps.
    let on_bound_limits: Vec<Value> = (0..state::limits::MAX_SERVICE_ENVIRONMENT_CAPACITY_LIMITS)
        .map(|index| json!({"maxCapacity": 1, "capacityUnit": format!("ml.unit{index}.large")}))
        .collect();
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-bounds",
                "serviceEnvironmentType": SAGEMAKER_TRAINING,
                "capacityLimits": on_bound_limits,
            }),
        )
        .await;
    let mut order: Vec<Value> = Vec::new();
    for index in 0..state::limits::MAX_SERVICE_ENVIRONMENT_ORDER_ENTRIES {
        let name = format!("se-bounds-{index}");
        batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": &name,
                    "serviceEnvironmentType": SAGEMAKER_TRAINING,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
        order.push(json!({"order": index + 1, "serviceEnvironment": name}));
    }
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-bounds",
                "priority": 1,
                "jobQueueType": SAGEMAKER_TRAINING,
                "serviceEnvironmentOrder": order.clone(),
            }),
        )
        .await;
    // A payload ON the byte maximum is accepted, and the job it mints is weighed the same
    // way as any other.
    let prefix =
        format!(r#"{{"ResourceConfig":{{"InstanceType":"{SMALL_INSTANCE_TYPE}","InstanceCount":1}},"N":""#);
    let suffix = r#""}"#;
    let filler = state::limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES - prefix.len() - suffix.len();
    let on_bound_payload = format!("{prefix}{}{suffix}", "x".repeat(filler));
    assert_eq!(
        on_bound_payload.len(),
        state::limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES,
        "case the payload on the maximum: the row must sit exactly on the bound, got {} bytes",
        on_bound_payload.len()
    );
    let accepted = batch
        .ok(
            "/v1/submitservicejob",
            json!({
                "jobName": "sj-bounds",
                "jobQueue": "jq-bounds",
                "serviceJobType": SAGEMAKER_TRAINING,
                "serviceRequestPayload": &on_bound_payload,
            }),
        )
        .await;
    assert!(
        accepted["jobId"].as_str().is_some_and(|id| !id.is_empty()),
        "case on every maximum: the submit is accepted, got {accepted}"
    );

    // One PAST each maximum, each refused with the modelled ClientException and persisting
    // nothing.
    let mut too_many_limits = on_bound_limits.clone();
    too_many_limits.push(json!({"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}));
    let mut too_long_order = order.clone();
    too_long_order.push(json!({"order": 99, "serviceEnvironment": "se-bounds"}));
    let too_long_payload = format!(
        r#"{{"Note":"{}"}}"#,
        "x".repeat(state::limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES)
    );
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "capacityLimits one past the maximum, on a create",
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-too-many-limits",
                "serviceEnvironmentType": SAGEMAKER_TRAINING,
                "capacityLimits": too_many_limits,
            }),
        ),
        (
            "capacityLimits one past the maximum, on an update",
            "/v1/updateserviceenvironment",
            json!({
                "serviceEnvironment": "se-bounds",
                "capacityLimits": too_many_limits,
            }),
        ),
        (
            "serviceEnvironmentOrder one past the maximum, on a create",
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-too-long-order",
                "priority": 1,
                "jobQueueType": SAGEMAKER_TRAINING,
                "serviceEnvironmentOrder": too_long_order,
            }),
        ),
        (
            "serviceEnvironmentOrder one past the maximum, on an update",
            "/v1/updatejobqueue",
            json!({"jobQueue": "jq-bounds", "serviceEnvironmentOrder": too_long_order}),
        ),
        (
            "serviceRequestPayload past the maximum",
            "/v1/submitservicejob",
            json!({
                "jobName": "sj-too-long-payload",
                "jobQueue": "jq-bounds",
                "serviceJobType": SAGEMAKER_TRAINING,
                "serviceRequestPayload": &too_long_payload,
            }),
        ),
    ];
    for (case, uri, body) in rows {
        let (status, refused) = batch.call(uri, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {refused}"
        );
        assert!(
            error_message(&refused).contains("at most"),
            "case {case}: the refusal states the maximum, got {refused}"
        );
    }
}

/// RULE: placement decides nothing about ACCEPTANCE — a submit to a queue whose
/// environments cannot hold it is still the 200 that mints the job, while the refusals the
/// operation models are unchanged by the published capacity.
#[tokio::test]
async fn capacity_never_turns_a_submit_into_a_refusal() {
    let batch = Batch::new();
    let queue = environment_and_queue(
        &batch,
        "accepts",
        json!([{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}]),
    )
    .await;
    let payload = json!({
        "ResourceConfig": {"InstanceType": SMALL_INSTANCE_TYPE, "InstanceCount": 32},
    })
    .to_string();

    let accepted = batch
        .ok(
            "/v1/submitservicejob",
            json!({
                "jobName": "sj-accepted-over-capacity",
                "jobQueue": &queue,
                "serviceJobType": SAGEMAKER_TRAINING,
                "serviceRequestPayload": &payload,
            }),
        )
        .await;
    assert!(
        accepted["jobId"].as_str().is_some_and(|id| !id.is_empty()),
        "case over capacity: a job the environment cannot hold is still accepted, got \
         {accepted}"
    );

    // A queue that resolves to nothing is still the modelled refusal, whatever the
    // capacity anything publishes.
    let (status, refused) = batch
        .call(
            "/v1/submitservicejob",
            json!({
                "jobName": "sj-absent-queue",
                "jobQueue": "jq-not-created",
                "serviceJobType": SAGEMAKER_TRAINING,
                "serviceRequestPayload": &payload,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an absent queue: expected the modelled ClientException 400, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case an absent queue: expected the modelled ClientException, got {refused}"
    );
    assert!(
        error_message(&refused).contains("jq-not-created"),
        "case an absent queue: the refusal names the reference the request gave, got \
         {refused}"
    );
}
