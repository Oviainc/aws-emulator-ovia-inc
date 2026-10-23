// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `UpdateJobQueue`, over the wire.
//!
//! Every case drives real HTTP (restJson1) through the router, so what is asserted is
//! the response a `batch` client gets, and every update is read back through
//! `DescribeJobQueues` — the persisted record is the only thing either operation answers
//! from.
//!
//! The rules under test, each over a SET of inputs:
//!
//! * an update applies exactly the members the request carries and leaves every member it
//!   omits unchanged (the table in [`update_applies_only_the_members_the_request_carries`]
//!   varies which member is present, its value, the number of entries in a list, and the
//!   form — name or ARN — each entry is written in);
//! * the queue is addressed by name OR by ARN, as `jobQueue` documents;
//! * a `computeEnvironmentOrder` entry is stored normalised to the compute environment's
//!   ARN, whichever form the request used, and a `serviceEnvironmentOrder` entry to the
//!   service environment's ARN, with neither order ever joining the other;
//! * a refused update answers `ClientException` (HTTP 400) and writes NOTHING
//!   ([`refused_updates_change_nothing`]);
//! * `jobStateTimeLimitActions$maxTimeSeconds` is accepted on each published bound and
//!   refused one past either of them;
//! * an accepted update leaves the queue in `UPDATING`, and the CLOCK — not a read —
//!   moves it back to `VALID`;
//! * a queue whose state is `DISABLED` accepts no new jobs, the jobs already in it keep
//!   advancing, and enabling it again accepts jobs.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, describe_job, job_definition, job_queue, Batch,
};

/// The envelope every `ClientException` this service raises is worded in
/// (`state::client_exception_message`).
const REFUSAL_PREFIX: &str = "Error executing request, Exception : ";

/// The one `JobQueueDetail` for `reference` (a queue name or ARN).
async fn describe_queue(batch: &Batch, reference: &str) -> Value {
    let body = batch
        .ok("/v1/describejobqueues", json!({"jobQueues": [reference]}))
        .await;
    let queues = body["jobQueues"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        queues.len(),
        1,
        "case describe {reference}: expected exactly one JobQueueDetail, got {body}"
    );
    queues[0].clone()
}

/// One row of the update table: the request's members beyond `jobQueue`, and the queue
/// members that must read back afterwards.
struct Case {
    /// What this row varies, named in every assertion.
    name: &'static str,
    /// Whether the queue is addressed by name (`false`) or by ARN (`true`).
    by_arn: bool,
    /// The update's members, added to `{"jobQueue": ...}`.
    patch: Value,
    /// The `state` the queue must report afterwards.
    state: &'static str,
    /// The `priority` the queue must report afterwards.
    priority: i64,
    /// The compute environment NAMES the stored order must report, in order, as ARNs.
    environments: &'static [&'static str],
    /// The `maxTimeSeconds` of the stored `jobStateTimeLimitActions`, or `None` when the
    /// queue must still carry none.
    max_time_seconds: Option<i64>,
}

/// An update applies the members it carries and nothing else: the table varies WHICH
/// member is present, its value, how many entries a list has and whether an entry names a
/// compute environment by name or by ARN. Every row starts from an identical queue
/// (priority 1, `ENABLED`, one compute environment, no time-limit actions).
#[tokio::test]
async fn update_applies_only_the_members_the_request_carries() {
    let batch = Batch::new();
    let first = compute_environment(&batch, "ce-first").await;
    let second = compute_environment(&batch, "ce-second").await;
    assert_eq!(
        second,
        arn("compute-environment/ce-second"),
        "case setup: expected the minted ARN, got {second:?}"
    );

    // `maxTimeSeconds`' published bounds, from the member's doc: "The minimum value is
    // 600 (10 minutes) and the maximum value is 86,400 (24 hours)."
    let min_seconds = i64::from(state::MIN_JOB_STATE_TIME_LIMIT_SECONDS);
    let max_seconds = i64::from(state::MAX_JOB_STATE_TIME_LIMIT_SECONDS);

    let cases = vec![
        Case {
            name: "no member beyond jobQueue: nothing changes",
            by_arn: false,
            patch: json!({}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "state only, DISABLED",
            by_arn: false,
            patch: json!({"state": "DISABLED"}),
            state: "DISABLED",
            priority: 1,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "state only, ENABLED (already enabled)",
            by_arn: false,
            patch: json!({"state": "ENABLED"}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "priority only, zero",
            by_arn: false,
            patch: json!({"priority": 0}),
            state: "ENABLED",
            priority: 0,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "priority only, raised",
            by_arn: false,
            patch: json!({"priority": 500}),
            state: "ENABLED",
            priority: 500,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "priority only, negative",
            by_arn: false,
            patch: json!({"priority": -7}),
            state: "ENABLED",
            priority: -7,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "state and priority together",
            by_arn: false,
            patch: json!({"state": "DISABLED", "priority": 42}),
            state: "DISABLED",
            priority: 42,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "computeEnvironmentOrder replaced, one entry by name",
            by_arn: false,
            patch: json!({"computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-second"}]}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-second"],
            max_time_seconds: None,
        },
        Case {
            name: "computeEnvironmentOrder replaced, two entries by ARN",
            by_arn: false,
            patch: json!({"computeEnvironmentOrder": [
                {"order": 1, "computeEnvironment": second},
                {"order": 2, "computeEnvironment": first},
            ]}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-second", "ce-first"],
            max_time_seconds: None,
        },
        Case {
            name: "jobStateTimeLimitActions at the documented minimum",
            by_arn: false,
            patch: json!({"jobStateTimeLimitActions": [{
                "reason": "MISCONFIGURATION:COMPUTE_ENVIRONMENT_MAX_RESOURCE",
                "state": "RUNNABLE",
                "maxTimeSeconds": min_seconds,
                "action": "CANCEL",
            }]}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-first"],
            max_time_seconds: Some(min_seconds),
        },
        Case {
            name: "jobStateTimeLimitActions at the documented maximum",
            by_arn: false,
            patch: json!({"jobStateTimeLimitActions": [{
                "reason": "MISCONFIGURATION:JOB_RESOURCE_REQUIREMENT",
                "state": "RUNNABLE",
                "maxTimeSeconds": max_seconds,
                "action": "CANCEL",
            }]}),
            state: "ENABLED",
            priority: 1,
            environments: &["ce-first"],
            max_time_seconds: Some(max_seconds),
        },
        Case {
            name: "addressed by ARN rather than by name",
            by_arn: true,
            patch: json!({"priority": 9, "state": "DISABLED"}),
            state: "DISABLED",
            priority: 9,
            environments: &["ce-first"],
            max_time_seconds: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("q-update-{index}");
        let queue_arn = job_queue(&batch, &name, "ce-first").await;
        let reference = if case.by_arn {
            queue_arn.clone()
        } else {
            name.clone()
        };

        let mut body = json!({"jobQueue": reference});
        for (key, value) in case.patch.as_object().expect("the patch is an object") {
            body[key] = value.clone();
        }
        let updated = batch.ok("/v1/updatejobqueue", body).await;
        assert_eq!(
            updated["jobQueueName"].as_str(),
            Some(name.as_str()),
            "case {}: expected the queue name back, got {updated}",
            case.name
        );
        assert_eq!(
            updated["jobQueueArn"].as_str(),
            Some(queue_arn.as_str()),
            "case {}: expected the queue ARN back, got {updated}",
            case.name
        );

        let queue = describe_queue(&batch, &name).await;
        assert_eq!(
            queue["state"].as_str(),
            Some(case.state),
            "case {}: expected state {:?}, got {queue}",
            case.name,
            case.state
        );
        assert_eq!(
            queue["priority"].as_i64(),
            Some(case.priority),
            "case {}: expected priority {}, got {queue}",
            case.name,
            case.priority
        );
        let expected_order: Vec<String> = case
            .environments
            .iter()
            .map(|environment| arn(&format!("compute-environment/{environment}")))
            .collect();
        let stored_order: Vec<String> = queue["computeEnvironmentOrder"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                entry["computeEnvironment"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            stored_order, expected_order,
            "case {}: expected the order normalised to {expected_order:?}, got {stored_order:?}",
            case.name
        );
        let stored_limit = queue["jobStateTimeLimitActions"]
            .as_array()
            .and_then(|actions| actions.first())
            .and_then(|action| action["maxTimeSeconds"].as_i64());
        assert_eq!(
            stored_limit, case.max_time_seconds,
            "case {}: expected jobStateTimeLimitActions maxTimeSeconds {:?}, got {queue}",
            case.name, case.max_time_seconds
        );
        // Nothing the update did not name was invented.
        assert!(
            queue["schedulingPolicyArn"].is_null(),
            "case {}: a FIFO queue must stay FIFO, got {queue}",
            case.name
        );
    }
}

/// Every refusal: the modelled 400, and a queue that still reads back exactly as it was
/// created. The rows vary the KIND of bad request — an unknown queue by name and by ARN,
/// an unresolvable or empty `computeEnvironmentOrder`, a service environment nothing
/// holds, the fair-share policy rules, and `maxTimeSeconds` one past each published bound.
#[tokio::test]
async fn refused_updates_change_nothing() {
    let batch = Batch::new();
    compute_environment(&batch, "ce-only").await;
    let queue_arn = job_queue(&batch, "q-refusals", "ce-only").await;
    // Let the create's own CREATING -> VALID move fall due BEFORE the snapshot is taken,
    // so the detail compared below cannot change for a reason this test is not about.
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
    let before = describe_queue(&batch, "q-refusals").await;
    assert_eq!(
        before["status"].as_str(),
        Some(state::STATUS_VALID),
        "case the snapshot: expected a settled queue, got {before}"
    );

    let min_seconds = i64::from(state::MIN_JOB_STATE_TIME_LIMIT_SECONDS);
    let max_seconds = i64::from(state::MAX_JOB_STATE_TIME_LIMIT_SECONDS);
    let time_limit_action = |seconds: i64| {
        json!([{
            "reason": "MISCONFIGURATION:COMPUTE_ENVIRONMENT_MAX_RESOURCE",
            "state": "RUNNABLE",
            "maxTimeSeconds": seconds,
            "action": "CANCEL",
        }])
    };

    let cases: Vec<(&str, Value)> = vec![
        ("unknown queue name", json!({"jobQueue": "q-nope"})),
        (
            "unknown queue ARN",
            json!({"jobQueue": arn("job-queue/q-nope")}),
        ),
        (
            "computeEnvironmentOrder names an unknown environment",
            json!({
                "jobQueue": "q-refusals",
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-nope"}],
            }),
        ),
        (
            "computeEnvironmentOrder is empty",
            json!({"jobQueue": "q-refusals", "computeEnvironmentOrder": []}),
        ),
        (
            "serviceEnvironmentOrder names an environment nothing holds",
            json!({
                "jobQueue": "q-refusals",
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": "se-nope"}],
            }),
        ),
        (
            "a fair-share policy added to a FIFO queue",
            json!({
                "jobQueue": "q-refusals",
                "schedulingPolicyArn": arn("scheduling-policy/fair"),
            }),
        ),
        (
            "a fair-share policy removed",
            json!({"jobQueue": "q-refusals", "schedulingPolicyArn": ""}),
        ),
        (
            "maxTimeSeconds one below the documented minimum",
            json!({
                "jobQueue": "q-refusals",
                "jobStateTimeLimitActions": time_limit_action(min_seconds - 1),
            }),
        ),
        (
            "maxTimeSeconds one above the documented maximum",
            json!({
                "jobQueue": "q-refusals",
                "jobStateTimeLimitActions": time_limit_action(max_seconds + 1),
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answered) = batch.call("/v1/updatejobqueue", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(REFUSAL_PREFIX),
            "case {case}: expected a ClientException worded like Batch's, got {answered}"
        );

        let after = describe_queue(&batch, &queue_arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must write nothing, got {after} instead of {before}"
        );
    }
}

/// `serviceEnvironmentOrder` resolves against the service environments
/// `CreateServiceEnvironment` stored, exactly as `computeEnvironmentOrder` resolves
/// against compute environments: each entry is stored as the environment's ARN whichever
/// form named it. And `serviceEnvironmentOrder`'s own doc — "A job queue can't have both
/// a serviceEnvironmentOrder and a computeEnvironmentOrder field" — is enforced on the
/// queue the update would leave behind, in both directions.
#[tokio::test]
async fn service_environment_order_resolves_to_arns_and_never_joins_a_compute_order() {
    let batch = Batch::new();
    // `ServiceEnvironmentType`'s only modelled value.
    let environment_type = "SAGEMAKER_TRAINING";
    for name in ["se-one", "se-two"] {
        batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": environment_type,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
    }
    compute_environment(&batch, "ce-mixed").await;

    // A queue over a SERVICE environment, created by name and stored as the ARN.
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "q-service",
                "priority": 1,
                "jobQueueType": environment_type,
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": "se-one"}],
            }),
        )
        .await;
    let created = describe_queue(&batch, "q-service").await;
    assert_eq!(
        created["serviceEnvironmentOrder"][0]["serviceEnvironment"].as_str(),
        Some(arn("service-environment/se-one").as_str()),
        "case create over a service environment: expected the order normalised to the ARN, got {created}"
    );

    // An update replaces the order, naming one entry by ARN and one by name.
    batch
        .ok(
            "/v1/updatejobqueue",
            json!({
                "jobQueue": "q-service",
                "serviceEnvironmentOrder": [
                    {"order": 1, "serviceEnvironment": arn("service-environment/se-two")},
                    {"order": 2, "serviceEnvironment": "se-one"},
                ],
            }),
        )
        .await;
    let updated = describe_queue(&batch, "q-service").await;
    let stored: Vec<String> = updated["serviceEnvironmentOrder"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["serviceEnvironment"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let expected = vec![
        arn("service-environment/se-two"),
        arn("service-environment/se-one"),
    ];
    assert_eq!(
        stored, expected,
        "case update the service-environment order: expected {expected:?}, got {stored:?}"
    );

    // Neither order can be added beside the other, whichever one the queue already has,
    // and an empty list names no environment at all.
    let both_ways: Vec<(&str, Value)> = vec![
        (
            "a compute order added to a service-environment queue",
            json!({
                "jobQueue": "q-service",
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-mixed"}],
            }),
        ),
        (
            "an empty service-environment order",
            json!({"jobQueue": "q-service", "serviceEnvironmentOrder": []}),
        ),
    ];
    for (case, body) in both_ways {
        let (status, answered) = batch.call("/v1/updatejobqueue", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answered}"
        );
    }

    // And the other direction: a queue created over a compute environment.
    job_queue(&batch, "q-compute", "ce-mixed").await;
    let (status, answered) = batch
        .call(
            "/v1/updatejobqueue",
            json!({
                "jobQueue": "q-compute",
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": "se-one"}],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a service order added to a compute-environment queue: expected 400, got {status} {answered}"
    );
    let unchanged = describe_queue(&batch, "q-compute").await;
    assert!(
        unchanged["serviceEnvironmentOrder"].is_null(),
        "case a refused service order: nothing may be written, got {unchanged}"
    );
}

/// An accepted update puts the queue in `UPDATING`, and only the CLOCK moves it back to
/// `VALID`: two reads at the same instant answer identically, and the values the update
/// wrote survive the move.
#[tokio::test]
async fn an_accepted_update_leaves_updating_until_the_clock_moves_it() {
    let batch = Batch::new();
    compute_environment(&batch, "ce-clock").await;
    job_queue(&batch, "q-clock", "ce-clock").await;

    // Let the create's own CREATING -> VALID move fall due first.
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
    let valid = describe_queue(&batch, "q-clock").await;
    assert_eq!(
        valid["status"].as_str(),
        Some(state::STATUS_VALID),
        "case before the update: expected VALID, got {valid}"
    );

    batch
        .ok(
            "/v1/updatejobqueue",
            json!({"jobQueue": "q-clock", "priority": 77, "state": "DISABLED"}),
        )
        .await;

    let updating = describe_queue(&batch, "q-clock").await;
    assert_eq!(
        updating["status"].as_str(),
        Some(state::STATUS_UPDATING),
        "case straight after the update: expected UPDATING, got {updating}"
    );
    // A read changes nothing a later read sees.
    let again = describe_queue(&batch, "q-clock").await;
    assert_eq!(
        again, updating,
        "case two reads at the same instant: expected the same detail, got {again} and {updating}"
    );

    tokio::time::sleep(Duration::from_millis(span * 3)).await;
    let settled = describe_queue(&batch, "q-clock").await;
    assert_eq!(
        settled["status"].as_str(),
        Some(state::STATUS_VALID),
        "case after the owed span: expected VALID, got {settled}"
    );
    assert_eq!(
        settled["priority"].as_i64(),
        Some(77),
        "case after the owed span: expected the updated priority, got {settled}"
    );
    assert_eq!(
        settled["state"].as_str(),
        Some(state::STATE_DISABLED),
        "case after the owed span: expected the updated state, got {settled}"
    );
}

/// `JQState`: "If the job queue state is ENABLED, it can accept jobs. If the job queue
/// state is DISABLED, new jobs can't be added to the queue, but jobs already in the queue
/// can finish." So a submit is accepted or refused according to the state the update
/// wrote, in both directions, and the job submitted before the queue was disabled still
/// reaches a terminal status.
#[tokio::test]
async fn a_disabled_queue_accepts_no_new_jobs() {
    let batch = Batch::new();
    compute_environment(&batch, "ce-gate").await;
    let queue_arn = job_queue(&batch, "q-gate", "ce-gate").await;
    job_definition(&batch, "jd-gate", container_properties()).await;

    let submit = |name: &'static str| {
        batch.call(
            "/v1/submitjob",
            json!({"jobName": name, "jobQueue": "q-gate", "jobDefinition": "jd-gate"}),
        )
    };

    // ENABLED (the create's default) accepts.
    let (status, accepted) = submit("before").await;
    assert_eq!(
        status, 200,
        "case submit to an ENABLED queue: expected 200, got {status} {accepted}"
    );
    let queued = accepted["jobId"].as_str().unwrap_or_default().to_string();

    // Each row updates the state and then submits once: the state the queue carries is
    // what decides the answer, whichever direction the update moved it.
    for (case, state_value, expected) in [
        ("DISABLED refuses", "DISABLED", 400),
        ("ENABLED again accepts", "ENABLED", 200),
        ("DISABLED refuses again", "DISABLED", 400),
    ] {
        batch
            .ok(
                "/v1/updatejobqueue",
                json!({"jobQueue": queue_arn, "state": state_value}),
            )
            .await;
        let (status, answered) = submit("after").await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} {answered}"
        );
        if status == 400 {
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with(REFUSAL_PREFIX),
                "case {case}: expected a ClientException worded like Batch's, got {answered}"
            );
        }
    }

    // "jobs already in the queue can finish": the job submitted while the queue was
    // enabled keeps advancing on the clock.
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 6)).await;
    let detail = describe_job(&batch, &queued).await;
    let status = detail["status"].as_str().unwrap_or_default();
    assert!(
        state::is_terminal_status(status),
        "case a job already in a disabled queue: expected a terminal status, got {detail}"
    );
}
