// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW AWS Batch words a refusal, over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client branches on. Each test asserts a
//! RULE over a table of inputs; the request the issue recorded is one row of each.
//!
//! The rules under test:
//!
//! * a call whose identifier resolves to NO stored resource is refused with the modelled
//!   `ClientException` (HTTP 400) whose message is the bare
//!   `<the ARN the identifier resolves to> does not exist` — the ARN built from the
//!   request (region and account of the call), never the plain name and never inside the
//!   frontend envelope; the job-queue resolver terminates that sentence with a period;
//! * an operation that REMOVES a resource is idempotent in its identifier: one that
//!   resolves to nothing is the same empty success, not a not-found refusal;
//! * a refusal AWS answers INSIDE its frontend envelope carries the id of the call being
//!   refused: `Error executing request, Exception : <detail>, RequestId: <id>`, where the
//!   id is a freshly minted request id — distinct for every call and never a placeholder;
//! * `DescribeJobs` honours the documented maximum of 100 job ids;
//! * `ListJobs` reports the selector condition that actually holds: NO selector is the
//!   missing-`jobQueue` refusal and more than one is the mutual-exclusion refusal;
//! * `SubmitJob` resolves the `jobDefinition` before the `jobQueue`, and a reference that
//!   names no revision and one that names a revision which is not `ACTIVE` are the same
//!   refusal;
//! * `CreateJobQueue` states the service-environment PRECONDITION for a
//!   `serviceEnvironmentOrder` entry it cannot attach, instead of naming the resource;
//! * `RegisterJobDefinition` validates each `nodeRangeProperties` entry's containers
//!   before the properties mutual-exclusion rule, naming the range it found empty.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, job_definition,
    job_queue, Batch,
};

/// The prefix of Batch's frontend envelope.
const ENVELOPE_PREFIX: &str = "Error executing request, Exception : ";

/// The segment the envelope names the refused call's request id in.
const ENVELOPE_REQUEST_ID: &str = ", RequestId: ";

/// Assert `message` is Batch's frontend envelope and answer `(detail, request id)`.
///
/// Shared with the other suites that assert an enveloped refusal, so the envelope's shape
/// is written down exactly once.
pub(crate) fn enveloped(case: &str, message: &str) -> (String, String) {
    let body = message.strip_prefix(ENVELOPE_PREFIX).unwrap_or_else(|| {
        panic!("case {case}: expected the message to start {ENVELOPE_PREFIX:?}, got {message:?}")
    });
    let (detail, request_id) = body.rsplit_once(ENVELOPE_REQUEST_ID).unwrap_or_else(|| {
        panic!("case {case}: expected a {ENVELOPE_REQUEST_ID:?} segment, got {message:?}")
    });
    assert_uuid_shaped(case, request_id);
    (detail.to_string(), request_id.to_string())
}

/// Assert `value` has the shape of an identifier this service mints: a lowercase
/// version-4 UUID, `8-4-4-4-12` hex. A placeholder such as `None` fails here.
pub(crate) fn assert_uuid_shaped(case: &str, value: &str) {
    let groups: Vec<usize> = value.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case}: expected an 8-4-4-4-12 UUID, got {value:?}"
    );
    assert!(
        value
            .chars()
            .all(|c| (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) || c == '-'),
        "case {case}: expected lowercase hex groups, got {value:?}"
    );
}

/// One resolver of this service: the call, the member its identifier arrives on, and the
/// ARN kind that identifier resolves to.
struct Resolver {
    label: &'static str,
    uri: &'static str,
    member: &'static str,
    /// The ARN resource segment of this resource kind, from the ARNs the service mints
    /// (`state::compute_environment_arn`, `state::job_queue_arn`, …).
    kind: &'static str,
    /// Whether the recorded sentence for this resolver ends with a period: the job-queue
    /// resolver's does (`<arn> does not exist.`), the others' do not.
    terminated: bool,
    /// The identifiers to try, which is what this table varies: name shapes, and a job
    /// id, with each one also sent as the full ARN it resolves to.
    identifiers: &'static [&'static str],
}

/// Every implemented operation that RESOLVES one identifier of a resource it must then
/// read or reconfigure. `DescribeConsumableResource`, `UpdateComputeEnvironment`,
/// `UpdateJobQueue` and `DescribeServiceJob` are the four the issue recorded; the
/// removal operations are NOT here, because a removal of something absent is already
/// done (see `removing_what_is_already_absent_is_the_same_success`).
fn resolvers() -> Vec<Resolver> {
    vec![
        Resolver {
            label: "UpdateComputeEnvironment",
            uri: "/v1/updatecomputeenvironment",
            member: "computeEnvironment",
            kind: "compute-environment",
            terminated: false,
            identifiers: &["ce-absent", "CE_Absent-2"],
        },
        Resolver {
            label: "DescribeConsumableResource",
            uri: "/v1/describeconsumableresource",
            member: "consumableResource",
            kind: "consumable-resource",
            terminated: false,
            identifiers: &["cr-absent", "cr_absent_2"],
        },
        Resolver {
            label: "UpdateJobQueue",
            uri: "/v1/updatejobqueue",
            member: "jobQueue",
            kind: "job-queue",
            terminated: true,
            identifiers: &["jq-absent", "JQ-absent-2"],
        },
        Resolver {
            label: "DescribeServiceJob",
            uri: "/v1/describeservicejob",
            member: "jobId",
            kind: "service-job",
            terminated: false,
            identifiers: &[
                "00000000-0000-0000-0000-000000000000",
                "ffffffff-ffff-4fff-bfff-ffffffffffff",
            ],
        },
    ]
}

/// RULE: a Batch call whose identifier resolves to no stored resource is refused with the
/// modelled `ClientException` (400) and the bare sentence
/// `<the ARN the identifier resolves to> does not exist` — built from the REQUEST, with no
/// envelope and no resource-kind prose around it.
#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_refused_with_its_resolved_arn() {
    let batch = Batch::new();
    for resolver in resolvers() {
        for identifier in resolver.identifiers {
            let expected_arn = arn(&format!("{}/{identifier}", resolver.kind));
            // Both forms the identifier may take: the plain name (or id) and the full ARN
            // it resolves to. Either way the refusal names the ARN.
            for (form, sent) in [("name", (*identifier).to_string()), ("arn", expected_arn.clone())]
            {
                let case = format!("{} by {form} {sent}", resolver.label);
                let (status, body) = batch
                    .call(resolver.uri, json!({resolver.member: sent}))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case}: expected the modelled ClientException 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ClientException",
                    "case {case}: expected the modelled ClientException, got {body}"
                );
                let suffix = if resolver.terminated { "." } else { "" };
                assert_eq!(
                    error_message(&body),
                    format!("{expected_arn} does not exist{suffix}"),
                    "case {case}: expected the bare resolved-ARN sentence, got {body}"
                );
            }
        }
    }
}

/// One refusal AWS answers inside its frontend envelope: the request that provokes it and
/// the detail the envelope must carry.
struct Enveloped {
    label: &'static str,
    uri: &'static str,
    body: Value,
    detail: &'static str,
}

/// RULE: an operation that REMOVES a resource is idempotent in its identifier — one that
/// resolves to nothing asks for a post-state that already holds, so the call answers its
/// empty success body, refuses nothing and stores nothing.
#[tokio::test]
async fn removing_what_is_already_absent_is_the_same_success() {
    let batch = Batch::new();
    // The rows span the removal operations and the identifier FORMS each one documents:
    // a plain name, a full ARN, and — for a job definition — `name:revision`.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "DeleteComputeEnvironment by name",
            "/v1/deletecomputeenvironment",
            "computeEnvironment",
            "ce-never".to_string(),
        ),
        (
            "DeleteComputeEnvironment by ARN",
            "/v1/deletecomputeenvironment",
            "computeEnvironment",
            arn("compute-environment/ce-never"),
        ),
        (
            "DeleteJobQueue by name",
            "/v1/deletejobqueue",
            "jobQueue",
            "jq-never".to_string(),
        ),
        (
            "DeleteJobQueue by ARN",
            "/v1/deletejobqueue",
            "jobQueue",
            arn("job-queue/jq-never"),
        ),
        (
            "DeregisterJobDefinition by name",
            "/v1/deregisterjobdefinition",
            "jobDefinition",
            "jd-never".to_string(),
        ),
        (
            "DeregisterJobDefinition by name:revision",
            "/v1/deregisterjobdefinition",
            "jobDefinition",
            "jd-never:1".to_string(),
        ),
        (
            "DeregisterJobDefinition by ARN",
            "/v1/deregisterjobdefinition",
            "jobDefinition",
            arn("job-definition/jd-never:3"),
        ),
    ];
    for (case, uri, member, identifier) in rows {
        let (status, body) = batch.call(uri, json!({member: &identifier})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the empty success, got {status} {body}"
        );
        assert!(
            body.get("__type").is_none() && body.get("message").is_none(),
            "case {case}: expected no error members, got {body}"
        );
    }
    {
        let store = batch.store();
        let store = store.lock();
        assert!(
            store.compute_environments.is_empty()
                && store.job_queues.is_empty()
                && store.job_definitions.is_empty(),
            "case every removal of an absent resource: nothing may be stored, got {} \
             compute environments, {} job queues and {} job definition revisions",
            store.compute_environments.len(),
            store.job_queues.len(),
            store.job_definitions.len()
        );
    }

    // The same on a resource that DID exist: the second removal of it is the same success
    // as the first, which is what makes a teardown repeatable.
    compute_environment(&batch, "ce-twice").await;
    batch
        .ok(
            "/v1/updatecomputeenvironment",
            json!({"computeEnvironment": "ce-twice", "state": "DISABLED"}),
        )
        .await;
    for attempt in 1..=2 {
        let (status, body) = batch
            .call(
                "/v1/deletecomputeenvironment",
                json!({"computeEnvironment": "ce-twice"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case delete ce-twice (attempt {attempt}): expected the empty success, \
             got {status} {body}"
        );
    }
    let (status, body) = batch
        .call(
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": "jd-twice"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deregister a name nothing registered: expected the empty success, \
         got {status} {body}"
    );
    let (_, registered) = job_definition(&batch, "jd-twice", container_properties()).await;
    assert_eq!(
        registered, 1,
        "case register after deregistering an absent name: the absent deregister must \
         have stored nothing, got revision {registered}"
    );
    for attempt in 1..=2 {
        let (status, body) = batch
            .call(
                "/v1/deregisterjobdefinition",
                json!({"jobDefinition": "jd-twice:1"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case deregister jd-twice:1 (attempt {attempt}): expected the empty success, \
             got {status} {body}"
        );
    }
}

/// RULE: an enveloped refusal names the id of the CALL being refused — a freshly minted
/// request id, distinct per call, never a placeholder.
#[tokio::test]
async fn an_enveloped_refusal_names_a_freshly_minted_request_id() {
    let batch = Batch::new();
    let rows = vec![
        Enveloped {
            label: "CreateComputeEnvironment MANAGED with no computeResources",
            uri: "/v1/createcomputeenvironment",
            body: json!({"computeEnvironmentName": "ce-managed", "type": "MANAGED"}),
            detail: "computeResources must be provided for a MANAGED compute environment",
        },
        Enveloped {
            label: "ListJobs with no selector",
            uri: "/v1/listjobs",
            body: json!({}),
            detail: "Job queue name is required.",
        },
        Enveloped {
            label: "RegisterJobDefinition type container with no properties object",
            uri: "/v1/registerjobdefinition",
            body: json!({"jobDefinitionName": "jd-bare", "type": "container"}),
            detail: "ECS ContainerProperties, ecsProperties, or eksProperties is required \
                     when using type container.",
        },
        Enveloped {
            label: "RegisterJobDefinition with an empty node range",
            uri: "/v1/registerjobdefinition",
            body: json!({
                "jobDefinitionName": "jd-empty-range",
                "type": "multinode",
                "nodeProperties": {
                    "numNodes": 2,
                    "mainNode": 0,
                    "nodeRangeProperties": [{"targetNodes": "0:1"}],
                },
            }),
            detail: "Must specify container for node range 0:1.",
        },
        Enveloped {
            label: "CreateComputeEnvironment UNMANAGED over Fargate compute resources",
            uri: "/v1/createcomputeenvironment",
            body: json!({
                "computeEnvironmentName": "ce-unmanaged-fargate",
                "type": "UNMANAGED",
                "computeResources": {
                    "type": "FARGATE",
                    "maxvCpus": 4,
                    "subnets": ["subnet-1"],
                    "securityGroupIds": ["sg-1"],
                },
            }),
            detail: "Cannot create an UNMANAGED Fargate Compute Environment.",
        },
        Enveloped {
            label: "CreateJobQueue with neither environment order",
            uri: "/v1/createjobqueue",
            body: json!({"jobQueueName": "q-no-order", "priority": 1}),
            detail: "computeEnvironmentOrder is required.",
        },
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for row in &rows {
        // Twice, so the id can be shown to be per CALL rather than per refusal.
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", row.label);
            let (status, body) = batch.call(row.uri, row.body.clone()).await;
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled ClientException 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ClientException",
                "case {case}: expected the modelled ClientException, got {body}"
            );
            let message = error_message(&body);
            let (detail, request_id) = enveloped(&case, &message);
            assert_eq!(
                detail, row.detail,
                "case {case}: expected the recorded detail, got {message:?}"
            );
            request_ids.push(request_id);
        }
    }

    let mut distinct = request_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "case every enveloped refusal: expected one request id per call, got {request_ids:?}"
    );
}

/// RULE: `DescribeJobs` accepts up to the documented maximum of job ids
/// (`DescribeJobsRequest$jobs` is "A list of up to 100 job IDs") and refuses a request
/// naming more, with the modelled `ClientException` inside the frontend envelope.
#[tokio::test]
async fn describe_jobs_honours_the_documented_job_id_maximum() {
    let batch = Batch::new();
    let maximum = state::limits::MAX_DESCRIBE_JOBS;
    // The bound itself, the values below it, and the first value past it.
    for count in [0, 1, maximum - 1, maximum] {
        let ids: Vec<String> = (0..count).map(|index| format!("job-{index}")).collect();
        let body = batch.ok("/v1/describejobs", json!({"jobs": ids})).await;
        assert_eq!(
            body["jobs"].as_array().map(Vec::len),
            Some(0),
            "case describe {count} unknown job ids: expected an empty jobs list, got {body}"
        );
    }
    for count in [maximum + 1, maximum * 2] {
        let ids: Vec<String> = (0..count).map(|index| format!("job-{index}")).collect();
        let (status, body) = batch.call("/v1/describejobs", json!({"jobs": ids})).await;
        assert_eq!(
            status, 400,
            "case describe {count} job ids: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case describe {count} job ids: expected the modelled ClientException, got {body}"
        );
        let message = error_message(&body);
        let (detail, _) = enveloped(&format!("describe {count} job ids"), &message);
        assert_eq!(
            detail,
            state::describe_jobs_bound_message(),
            "case describe {count} job ids: expected the recorded bound detail, got {message:?}"
        );
    }
}

/// RULE: `ListJobs` reports the selector condition that holds — none of
/// `jobQueue`/`arrayJobId`/`multiNodeJobId` is the missing-`jobQueue` refusal, and more
/// than one is the mutual-exclusion refusal; exactly one is accepted.
#[tokio::test]
async fn list_jobs_reports_the_selector_condition_that_holds() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-selectors").await;
    job_queue(&batch, "q-selectors", &environment).await;

    let refused = vec![
        ("no selector at all", json!({}), "Job queue name is required."),
        (
            "jobQueue and arrayJobId",
            json!({"jobQueue": "q-selectors", "arrayJobId": "a-1"}),
            "Only one of job queue, array job id and multinode job id can be set.",
        ),
        (
            "jobQueue and multiNodeJobId",
            json!({"jobQueue": "q-selectors", "multiNodeJobId": "m-1"}),
            "Only one of job queue, array job id and multinode job id can be set.",
        ),
        (
            "arrayJobId and multiNodeJobId",
            json!({"arrayJobId": "a-1", "multiNodeJobId": "m-1"}),
            "Array job id and multinode job id cannot both be set.",
        ),
        (
            "all three selectors",
            json!({"jobQueue": "q-selectors", "arrayJobId": "a-1", "multiNodeJobId": "m-1"}),
            "Array job id and multinode job id cannot both be set.",
        ),
    ];
    for (case, request, detail) in refused {
        let (status, body) = batch.call("/v1/listjobs", request).await;
        assert_eq!(
            status, 400,
            "case list with {case}: expected 400, got {status} {body}"
        );
        let message = error_message(&body);
        let (found, _) = enveloped(&format!("list with {case}"), &message);
        assert_eq!(
            found, detail,
            "case list with {case}: expected the condition that holds, got {message:?}"
        );
    }

    // Exactly one selector is the accepted form, whichever of the three it is.
    for (case, request) in [
        ("jobQueue only", json!({"jobQueue": "q-selectors"})),
        ("arrayJobId only", json!({"arrayJobId": "a-1"})),
        ("multiNodeJobId only", json!({"multiNodeJobId": "m-1"})),
    ] {
        let (status, body) = batch.call("/v1/listjobs", request).await;
        assert_eq!(
            status, 200,
            "case list with {case}: expected 200, got {status} {body}"
        );
        assert!(
            body["jobSummaryList"].is_array(),
            "case list with {case}: expected a jobSummaryList, got {body}"
        );
    }
}

/// RULE: `SubmitJob` resolves the `jobDefinition` BEFORE the `jobQueue`, and the two ways
/// a reference fails to name a runnable revision — nothing stored, and a revision that is
/// not `ACTIVE` — are the same refusal.
#[tokio::test]
async fn submit_job_resolves_the_job_definition_before_the_queue() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-precedence").await;
    job_queue(&batch, "q-precedence", &environment).await;
    let (active_arn, _) = job_definition(&batch, "jd-active", container_properties()).await;
    let (inactive_arn, _) = job_definition(&batch, "jd-inactive", container_properties()).await;
    let deregistered = batch
        .call(
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": "jd-inactive:1"}),
        )
        .await;
    assert_eq!(
        deregistered.0, 200,
        "case deregister jd-inactive: expected 200, got {deregistered:?}"
    );

    /// The refusal a row expects: the job definition's (naming the reference) or the
    /// queue's own.
    enum Expected {
        /// `JobDefinition name <reference> does not exist or is not in ACTIVE status.`
        JobDefinition,
        /// The queue's refusal, which names the queue and not the job definition.
        JobQueue,
    }

    // Each row varies the reference FORM (bare name, `name:revision`, ARN) and which of
    // the two resources is resolvable.
    let absent_definition_arn = arn("job-definition/jd-absent:1");
    let rows: Vec<(&str, String, &str, Expected)> = vec![
        (
            "neither the definition nor the queue exists",
            "jd-absent".to_string(),
            "q-absent",
            Expected::JobDefinition,
        ),
        (
            "an absent definition by name:revision, queue absent too",
            "jd-absent:7".to_string(),
            "q-absent",
            Expected::JobDefinition,
        ),
        (
            "an absent definition by ARN, with a queue that exists",
            absent_definition_arn,
            "q-precedence",
            Expected::JobDefinition,
        ),
        (
            "a revision of a stored name that was never registered",
            "jd-active:9".to_string(),
            "q-precedence",
            Expected::JobDefinition,
        ),
        (
            "a deregistered (INACTIVE) revision by ARN",
            inactive_arn.clone(),
            "q-precedence",
            Expected::JobDefinition,
        ),
        (
            "a deregistered name with no ACTIVE revision left",
            "jd-inactive".to_string(),
            "q-precedence",
            Expected::JobDefinition,
        ),
        // Only once the definition resolves does the queue's own refusal surface.
        (
            "an ACTIVE definition and an absent queue",
            "jd-active".to_string(),
            "q-absent",
            Expected::JobQueue,
        ),
        (
            "an ACTIVE definition by ARN and an absent queue",
            active_arn.clone(),
            "q-absent",
            Expected::JobQueue,
        ),
    ];

    for (case, definition, queue, expected) in rows {
        let (status, body) = batch
            .call(
                "/v1/submitjob",
                json!({"jobName": "n", "jobQueue": queue, "jobDefinition": &definition}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case submit with {case}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case submit with {case}: expected the modelled ClientException, got {body}"
        );
        let message = error_message(&body);
        match expected {
            Expected::JobDefinition => assert_eq!(
                message,
                format!(
                    "JobDefinition name {definition} does not exist or is not in ACTIVE status."
                ),
                "case submit with {case}: expected the job-definition refusal, got {message:?}"
            ),
            Expected::JobQueue => assert!(
                !message.contains("JobDefinition") && message.contains(queue),
                "case submit with {case}: expected the queue's own refusal, got {message:?}"
            ),
        }
    }

    // The accepted row: both resolve, so the submit answers 200 and stores a job.
    let accepted = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "n-ok",
                "jobQueue": "q-precedence",
                "jobDefinition": "jd-active",
            }),
        )
        .await;
    assert!(
        accepted["jobId"].as_str().is_some_and(|id| !id.is_empty()),
        "case submit with both resolvable: expected a jobId, got {accepted}"
    );
}

/// RULE: a `serviceEnvironmentOrder` entry `CreateJobQueue` cannot attach is refused with
/// the service-environment PRECONDITION — one sentence covering "not created" and "not
/// valid" — rather than with a per-resource existence error.
#[tokio::test]
async fn create_job_queue_states_the_service_environment_precondition() {
    let batch = Batch::new();
    let created = batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-attached",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    let stored_arn = created["serviceEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rows vary the FORM of the unattachable reference (name, ARN), how many entries
    // the order carries and where the bad entry sits.
    let rows = vec![
        (
            "one entry naming an environment nothing created",
            json!([{"order": 1, "serviceEnvironment": "se-absent"}]),
        ),
        (
            "one entry naming an absent environment by ARN",
            json!([{"order": 1, "serviceEnvironment": arn("service-environment/se-absent")}]),
        ),
        (
            "two entries, the second of them absent",
            json!([
                {"order": 1, "serviceEnvironment": "se-attached"},
                {"order": 2, "serviceEnvironment": "se-absent-too"},
            ]),
        ),
    ];
    for (index, (case, order)) in rows.into_iter().enumerate() {
        let name = format!("q-precondition-{index}");
        let (status, body) = batch
            .call(
                "/v1/createjobqueue",
                json!({
                    "jobQueueName": &name,
                    "priority": 1,
                    "jobQueueType": "SAGEMAKER_TRAINING",
                    "serviceEnvironmentOrder": order,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case create with {case}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case create with {case}: expected the modelled ClientException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            state::SERVICE_ENVIRONMENTS_NOT_ATTACHABLE,
            "case create with {case}: expected the precondition sentence, got {body}"
        );
        // A refused create stores nothing a later caller could read back.
        let described = batch
            .ok("/v1/describejobqueues", json!({"jobQueues": [&name]}))
            .await;
        assert_eq!(
            described["jobQueues"].as_array().map(Vec::len),
            Some(0),
            "case create with {case}: a refused create must store nothing, got {described}"
        );
    }

    // The attachable environment is accepted, by name and by ARN, and reads back as the
    // ARN it resolved to.
    for (index, reference) in ["se-attached", stored_arn.as_str()].into_iter().enumerate() {
        let name = format!("q-attached-{index}");
        batch
            .ok(
                "/v1/createjobqueue",
                json!({
                    "jobQueueName": &name,
                    "priority": 1,
                    "jobQueueType": "SAGEMAKER_TRAINING",
                    "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": reference}],
                }),
            )
            .await;
        let described = batch
            .ok("/v1/describejobqueues", json!({"jobQueues": [&name]}))
            .await;
        assert_eq!(
            described["jobQueues"][0]["serviceEnvironmentOrder"][0]["serviceEnvironment"].as_str(),
            Some(stored_arn.as_str()),
            "case create by {reference}: expected the environment's ARN, got {described}"
        );
    }
}

/// The smallest `nodeProperties` for `ranges`: a two-node definition whose range entries
/// are exactly what the row carries.
fn node_properties(ranges: Value) -> Value {
    json!({"numNodes": 2, "mainNode": 0, "nodeRangeProperties": ranges})
}

/// RULE: every `nodeProperties.nodeRangeProperties` entry must carry the containers its
/// nodes run (`container`, `ecsProperties` or `eksProperties`); an entry carrying none is
/// refused naming THAT range, and the check precedes the properties mutual-exclusion rule.
#[tokio::test]
async fn register_job_definition_reports_the_empty_node_range_first() {
    let batch = Batch::new();
    let with_container = json!({"targetNodes": "0:0", "container": container_properties()});

    // The rows vary the `targetNodes` text of the empty range (the forms the member's own
    // doc documents: `n:n`, `:n`, `n:` and a nested wider range), how many entries the
    // list carries, where the empty one sits, and whether the request ALSO breaks the
    // mutual-exclusion rule.
    let rows = vec![
        (
            "one empty range 0:1",
            json!([{"targetNodes": "0:1"}]),
            false,
            "0:1",
        ),
        (
            "an empty range written :1, with an earlier range that has a container",
            json!([with_container.clone(), {"targetNodes": ":1"}]),
            false,
            ":1",
        ),
        (
            "an empty open-ended range 1:",
            json!([{"targetNodes": "1:"}, with_container.clone()]),
            false,
            "1:",
        ),
        (
            "an empty nested range 0:10 and containerProperties as well",
            json!([{"targetNodes": "0:10"}]),
            true,
            "0:10",
        ),
    ];
    for (index, (case, ranges, also_container_properties, range)) in rows.into_iter().enumerate() {
        let mut request = json!({
            "jobDefinitionName": format!("jd-range-{index}"),
            "type": "multinode",
            "nodeProperties": node_properties(ranges),
        });
        if also_container_properties {
            request["containerProperties"] = container_properties();
        }
        let (status, body) = batch.call("/v1/registerjobdefinition", request).await;
        assert_eq!(
            status, 400,
            "case register with {case}: expected 400, got {status} {body}"
        );
        let message = error_message(&body);
        let (detail, _) = enveloped(&format!("register with {case}"), &message);
        assert_eq!(
            detail,
            format!("Must specify container for node range {range}."),
            "case register with {case}: expected the empty node range to be named, \
             got {message:?}"
        );
    }

    // A node range that describes its containers is accepted — through `container` and
    // through `ecsProperties`, the two `NodeRangeProperty` members that carry them — and
    // the stored revision reads back through `DescribeJobDefinitions`.
    let accepted = vec![
        (
            "every range carries a container",
            json!([with_container.clone(), {"targetNodes": "1:1", "container": container_properties()}]),
        ),
        (
            "a range carrying ecsProperties instead of container",
            json!([{
                "targetNodes": "0:1",
                "ecsProperties": {"taskProperties": [{"containers": [{"image": "busybox"}]}]},
            }]),
        ),
    ];
    for (index, (case, ranges)) in accepted.into_iter().enumerate() {
        let name = format!("jd-range-ok-{index}");
        let body = batch
            .ok(
                "/v1/registerjobdefinition",
                json!({
                    "jobDefinitionName": &name,
                    "type": "multinode",
                    "nodeProperties": node_properties(ranges.clone()),
                }),
            )
            .await;
        assert_eq!(
            body["revision"].as_i64(),
            Some(1),
            "case register with {case}: expected revision 1, got {body}"
        );
        let described = batch
            .ok(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": &name}),
            )
            .await;
        let stored = &described["jobDefinitions"][0]["nodeProperties"]["nodeRangeProperties"];
        assert_eq!(
            stored.as_array().map(Vec::len),
            ranges.as_array().map(Vec::len),
            "case register with {case}: expected the node ranges to be stored, got {described}"
        );
    }
}
