Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|run#46|Timeout|time.sleep(10)
Issue-Commit: 547f40f49be2649465cead789446113d922f667f

# batch__events__sqs: a Batch Job State Change event is published for the initial SUBMITTED status, which AWS never emits

> _Note: this issue was identified against the code base at commit `547f40f49be2649465cead789446113d922f667f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause from the behavioral sweep, in the only failing check of this suite that is neither ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: SubmitJob publishes an `aws.batch` / "Batch Job State Change" event for the job's INITIAL `SUBMITTED` status.

AWS: AWS Batch job state change events are not in the Smithy model (the model only types the statuses themselves — `smithy_query(query="shape", shape="JobStatus")` = SUBMITTED | PENDING | RUNNABLE | STARTING | RUNNING | SUCCEEDED | FAILED), so the requirement is the documented eventing contract: the AWS Batch User Guide "Job state change events" states that an event is created "Anytime that an existing (previously submitted) job changes states" and is explicit that "Events aren't created for the initial job submission." The event stream for a job therefore begins at the FIRST transition out of submission (PENDING/RUNNABLE), with `source` = `aws.batch`, `detail-type` = "Batch Job State Change", on the account's default bus, and `detail.status` naming the state the job moved INTO. No envelope with `detail.status` == "SUBMITTED" is ever emitted for a job. Recorded behavior only for the per-field envelope shape; the SUBMITTED exclusion is the documented rule.

Emulator: the Batch slice treats entering SUBMITTED as a publishable transition like any other. `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_job.rs` stores the job with `status: batch::JOB_SUBMITTED` and then calls `batch::publish_job_state_changes(...)` in the same handler (its own header says "Entering `SUBMITTED` is a job state change like any other, so this call also publishes a `Batch Job State Change` event for it"), and `src/state.rs` confirms the queueing rule: "every write of a job's status — the `SUBMITTED` a submit stores, the moves the clock materialises, and the terminal status a cancel or a terminate writes — records a JobStateChange". The envelope is otherwise well formed (`EVENT_SOURCE` = "aws.batch", `JOB_STATE_CHANGE_DETAIL_TYPE` = "Batch Job State Change", `DEFAULT_EVENT_BUS` = "default", `job_state_change_detail` projects jobArn/jobName/jobId/jobQueue/jobDefinition/status/createdAt), so the spurious SUBMITTED event matches a rule pattern of {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<queue arn>]}} exactly as the real RUNNABLE event does and is delivered to that rule's SQS target.

Why: a consumer that drives bookkeeping off the Batch event stream sees one transition AWS never reports. Anything that counts transitions, or that treats each delivered event as a distinct state change (job dashboards, status-change notifiers, idempotency/dedup keyed on (jobId, status), state machines that fan out per event), acts on a phantom submission transition that has no counterpart in real AWS, and a consumer asserting the documented absence of a SUBMITTED event — as this check does — fails against the emulator while passing against AWS.

Fix: the job-state-change publication path, i.e. where a status write is turned into a queued JobStateChange. The initial status a submit stores must not enqueue a change; only transitions OUT of the submitted state (the clock's moves and the terminal status a cancel/terminate/queue-deletion writes) may. See the citations under Traces for the files and lines involved.

Traces: failing check `batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue` (verdict mismatch). Code citations: `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_job.rs` lines 20-24 (handler contract) and lines 374-426 (status written as SUBMITTED, then `publish_job_state_changes`); `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs` lines 48-56 (every status write, including the submit's SUBMITTED, is queued for publication), 771-789 (source/detail-type/bus constants), 832-852 (`job_state_change_detail`), 4113-4160 (`publish_job_state_changes` -> in-process PutEvents on the default bus).

## Evidence
Replay verdict for `batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue`: [mismatch] "Check timed out after 60.0s". Reported honestly: this check's own absence window is max(3L, 180s) (its `_MIN_HOLD_S` = 180) plus multiple 5s SQS long polls, so the 60s per-check cap is reached before the terminal assertions run; the timeout alone therefore does not localise the divergence.

What the exercised path does diverge on, deterministically and independently of that cap:

Expected (AWS): for a job submitted to an ENABLED queue on an UNMANAGED compute environment, the set of `detail.status` values delivered by a rule matching {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<watched queue arn>]}} contains RUNNABLE (and, after TerminateJob, FAILED) and NEVER contains SUBMITTED — the User Guide: "Events aren't created for the initial job submission." The check encodes exactly this: `assert "SUBMITTED" not in observed_statuses`, with the failure message "the User Guide is explicit that no event is created for the initial job submission, so no SUBMITTED event may appear".

Actual (emulator): SubmitJob stores `status = "SUBMITTED"` and, in the same handler, drains and publishes the queued state changes, so a well-formed `aws.batch` / "Batch Job State Change" envelope whose `detail.status` is "SUBMITTED" (and whose `detail.jobQueue` is the submitted queue's ARN, so it matches the rule) is delivered to the rule's SQS target before the RUNNABLE one. No HTTP status or error code diverges here — SubmitJob answers 200 as AWS does; the divergence is one extra event on the stream, so the observed status set is {SUBMITTED, RUNNABLE, ...} where AWS produces {RUNNABLE, ...}. Fixed text of the envelope (`source` = "aws.batch", `detail-type` = "Batch Job State Change") is correct; the variable part that must not exist at all is the member `detail.status` == "SUBMITTED" together with its `detail.jobId`/`detail.jobArn`/`detail.jobQueue` taken from the submitted job and queue.

## Steps to Reproduce
1. Create an UNMANAGED compute environment with a Batch service role and wait for it to reach VALID; create an ENABLED job queue on it and wait for VALID; register a trivial container job definition.
2. Create an SQS queue whose resource policy allows `events.amazonaws.com` to `sqs:SendMessage` on it, and an ENABLED rule on the DEFAULT event bus with pattern {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":["<job queue ARN>"]}} targeting that queue. Drain the queue to empty.
3. Call SubmitJob with that jobQueue and jobDefinition.
4. Receive from the SQS queue and collect every envelope with source == "aws.batch" and detail-type == "Batch Job State Change" for the returned jobId; read the set of `detail.status` values.
5. AWS: the set contains RUNNABLE (the first observable transition) and no SUBMITTED entry. Emulator: an envelope with `detail.status` == "SUBMITTED" is delivered for the submit itself, so the set contains SUBMITTED.
(The full suite check additionally holds a sibling-queue absence window and terminates the job; that part is unaffected by this cause and needs a per-check budget above 180s to run to completion.)
