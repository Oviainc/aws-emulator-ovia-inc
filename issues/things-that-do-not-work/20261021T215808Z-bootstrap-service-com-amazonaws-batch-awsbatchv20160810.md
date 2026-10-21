Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the AWS Batch emulator

_Filed: 2026-10-21T21:58:08Z_
_Operation: service-com-amazonaws-batch-awsbatchv20160810 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 48 handlers in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AWS Batch client workflow can run at all. This issue defines the FIRST, SMALL slice: the core job-submission lifecycle, implemented statefully end-to-end over the restJson1 protocol (SDK id `Batch`, sigv4 name `batch`).

### Happy path to implement (in order, 6 operations)

1. **CreateComputeEnvironment** — `POST /v1/createcomputeenvironment`. Input: `computeEnvironmentName` (required), `type` (MANAGED|UNMANAGED, required), `state` (DISABLED|ENABLED), `computeResources`, `serviceRole`, `tags`. Output: `computeEnvironmentName`, `computeEnvironmentArn`. Must mint and persist a stable ARN (`arn:aws:batch:<region>:<account>:compute-environment/<name>`), store the submitted config verbatim, and set `status=VALID`, `state` defaulting to `ENABLED`. Re-creating an existing name must fail with `ClientException` (HTTP 400), not overwrite.
2. **CreateJobQueue** — `POST /v1/createjobqueue`. Input: `jobQueueName` (required), `priority` (required), `state`, `computeEnvironmentOrder`, `jobQueueType`, `tags`. Output: `jobQueueName`, `jobQueueArn` (both required). Must validate that each `computeEnvironmentOrder[].computeEnvironment` resolves to a compute environment created in step 1 (otherwise `ClientException`), persist the queue with `status=VALID`, and mint `arn:aws:batch:<region>:<account>:job-queue/<name>`.
3. **RegisterJobDefinition** — `POST /v1/registerjobdefinition`. Input: `jobDefinitionName` (required), `type` (container|multinode, required), `containerProperties`, `parameters`, `retryStrategy`, `timeout`, `tags`. Output: `jobDefinitionName`, `jobDefinitionArn`, `revision` (all required). Revision must be a real per-name counter starting at 1 and incrementing on each re-register; the ARN must embed the revision (`.../job-definition/<name>:<revision>`) and prior revisions must stay retrievable with `status=ACTIVE`.
4. **SubmitJob** — `POST /v1/submitjob`. Input: `jobName` (required), `jobQueue` (required), `jobDefinition` (required), `parameters`, `containerOverrides`, `retryStrategy`, `timeout`, `tags`, `dependsOn`. Output: `jobId` (required), `jobName` (required), `jobArn`. Must resolve `jobQueue` by name OR ARN and `jobDefinition` by `name`, `name:revision` or ARN against persisted state — an unknown queue or definition is `ClientException` (HTTP 400), never a 200 with a fabricated id. Must mint a UUID `jobId`, persist the job record (merged definition + overrides, `createdAt` in epoch ms, `status=SUBMITTED`), and return the stored id.
5. **DescribeJobs** — `POST /v1/describejobs`. Input: `jobs` (list of ids, required). Output: `jobs` (list<JobDetail>). Must return one `JobDetail` per *known* id built from the persisted record — `jobId`, `jobName`, `jobQueue` (ARN), `jobDefinition`, `status`, `createdAt`, `startedAt`/`stoppedAt` when set, `container` with the effective (definition + override) command/image/vcpus/memory, `tags` — and silently omit unknown ids, as real Batch does. A job must be allowed to progress SUBMITTED → RUNNABLE → STARTING → RUNNING → SUCCEEDED on successive describes (a simple time- or call-driven state machine is acceptable), with `statusReason` and `container.exitCode` populated at terminal state.
6. **TerminateJob** — `POST /v1/terminatejob`. Input: `jobId` (required), `reason` (required). Output: no members. Must mutate the persisted job to `status=FAILED` with `statusReason` set to the supplied `reason` and `stoppedAt` set, so a following DescribeJobs observes the termination. Terminating an unknown `jobId` must be `ClientException` (HTTP 400).

Also implement **ListJobs** (`POST /v1/listjobs`) if cheap: it is how most clients observe queue contents — filter the persisted jobs by `jobQueue` and optional `jobStatus`, return `jobSummaryList` (required) of `JobSummary{jobId, jobName, jobArn, status, statusReason, createdAt, startedAt, stoppedAt}`, and honour `maxResults`/`nextToken`.

### Hard requirements

- NEVER STUBS: every one of these operations must compute its response from persisted state. No canned bodies, no echoing the input back, no minting an id that is not stored. A create followed by a describe must return exactly what was created.
- Names and ARNs must round-trip: anything accepted as input where the model allows "name or ARN" must resolve both ways.
- Errors must be the modeled shapes: `ClientException` (HTTP 400) for unknown/duplicate/invalid references, `ServerException` (HTTP 500) only for internal faults.
- Done = an integration script that creates a compute environment, creates a queue over it, registers a job definition, submits a job, describes the job until it reaches a terminal state, and terminates a second job, passes with correct bodies (not merely 2xx).

## State Model

Store all of this in the service's single shared `ComAmazonawsBatchAwsbatchv20160810State` (reached via `crate::state::service_state(&*self.engine)`):

- `compute_environments: HashMap<String /*name*/, ComputeEnvironment { name, arn, ce_type, state, status, status_reason, unmanaged_vcpus, compute_resources, service_role, tags, context }>` plus an `arn -> name` index.
- `job_queues: HashMap<String /*name*/, JobQueue { name, arn, state, status, status_reason, priority, scheduling_policy_arn, compute_environment_order: Vec<ComputeEnvironmentOrder>, job_queue_type, tags, job_state_time_limit_actions }>` plus an `arn -> name` index.
- `job_definitions: HashMap<String /*name*/, Vec<JobDefinitionRevision { name, arn, revision, def_type, status /*ACTIVE|INACTIVE*/, parameters, container_properties, node_properties, retry_strategy, timeout, propagate_tags, platform_capabilities, tags }>>` with `next_revision: HashMap<String, i32>` so revisions increment instead of being invented.
- `jobs: HashMap<String /*jobId*/, Job { job_id, job_arn, job_name, job_queue_arn, job_definition_arn, status, status_reason, created_at, started_at, stopped_at, attempts, parameters, container: ContainerDetail /*effective after overrides, incl. exit_code*/, retry_strategy, timeout, depends_on, share_identifier, scheduling_priority, tags }>` plus `jobs_by_queue: HashMap<String /*queue arn*/, Vec<String /*jobId ordered by created_at*/>>` to back ListJobs ordering and pagination.
- A shared `account_id`/`region` source used by every ARN minted, so ARNs are consistent across operations.

All other operations (consumable resources, scheduling policies, quota shares, service environments/service jobs, Update*/Delete*, tagging, GetJobQueueSnapshot, Cancel/Terminate batch variants) stay at 501 for this slice and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-batch-awsbatchv20160810/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-batch-awsbatchv20160810.json`) lists 48 operations, protocol `restJson1`, SDK id `Batch`, and gives the exact contracts cited above: CreateComputeEnvironment (line 40-45), CreateJobQueue (54-59), RegisterJobDefinition (260-265) with required `revision`, SubmitJob (267-272) with required `jobId`/`jobName`, DescribeJobs (162-167), ListJobs (213-219) with `@paginated(inputToken=nextToken, outputToken=nextToken, items=jobSummaryList)`, TerminateJob (288-293). Every operation declares only `ClientException (HTTP 400)` and `ServerException (HTTP 500)`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 48 handler files, including `submit_job.rs`, `describe_jobs.rs`, `create_job_queue.rs`, `create_compute_environment.rs`, `register_job_definition.rs` and `terminate_job.rs`, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. README.md confirms the scaffold answers 501 until the marker line is deleted and `handle` is written. Lifecycle ordering (compute environment → job queue → job definition → submit → describe → terminate) is well-established AWS Batch behavior.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with region/credentials set.
2. `aws batch create-compute-environment --compute-environment-name ce-demo --type UNMANAGED --state ENABLED` → expect 200 with `computeEnvironmentName` and a stable `computeEnvironmentArn`; currently HTTP 501 NotImplemented.
3. `aws batch create-job-queue --job-queue-name q-demo --priority 1 --compute-environment-order order=1,computeEnvironment=ce-demo` → expect 200 with `jobQueueArn`; an order referencing an unknown environment must return ClientException 400.
4. `aws batch register-job-definition --job-definition-name jd-demo --type container --container-properties '{"image":"busybox","command":["echo","hi"],"resourceRequirements":[{"type":"VCPU","value":"1"},{"type":"MEMORY","value":"512"}]}'` → expect `revision=1`; re-run and expect `revision=2`.
5. `aws batch describe-job-definitions --job-definition-name jd-demo` → expect both persisted revisions with `status=ACTIVE`.
6. `aws batch submit-job --job-name j1 --job-queue q-demo --job-definition jd-demo` → expect 200 with a stored `jobId`; submitting against `--job-queue nope` must return ClientException 400.
7. `aws batch describe-jobs --jobs <jobId>` → expect one JobDetail whose `jobName`, `jobQueue` ARN, `jobDefinition`, `createdAt` and effective container command match what was submitted, and whose `status` advances toward a terminal state on repeated calls.
8. `aws batch list-jobs --job-queue q-demo` → expect the submitted job in `jobSummaryList`.
9. `aws batch submit-job --job-name j2 --job-queue q-demo --job-definition jd-demo` then `aws batch terminate-job --job-id <jobId2> --reason "stop it"` then `aws batch describe-jobs --jobs <jobId2>` → expect `status=FAILED`, `statusReason="stop it"`, `stoppedAt` set.
10. `aws batch terminate-job --job-id does-not-exist --reason x` → expect ClientException 400, not 200/501.
