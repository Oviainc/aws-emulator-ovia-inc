Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 9f696bee74236d7bb111be1a5cc08ff08986f981

# Job-state notifications configured via Notification are never published to the SNS topic

_Filed: 2026-10-24T10:28:10Z_
_Operation: CreateJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `9f696bee74236d7bb111be1a5cc08ff08986f981`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a job created with a `Notification` whose `SnsTopicARN` names an SNS topic causes the service to publish a message to that topic whenever the job enters a state the notification selects — the states listed in `Notification$JobStatesToNotify`, or every state change when `Notification$NotifyAll` is true. Concretely: creating the job puts it in `JobState` `New` (a notifiable state), and `CancelJob` moves it to `Cancelled`; in the real service each such transition that the notification selects produces an SNS publish to the configured topic, observable by any subscriber of that topic (in this repo, the co-hosted SNS emulator and its subscriptions).

Actual: the Snowball emulator treats `Notification` as inert payload. `CreateJob` copies the whole `Notification` structure into `StoredJob.notification` so `DescribeJob` can echo it back, and `CancelJob` rewrites `job_state` to `Cancelled`, but nothing in the crate ever publishes anything: the crate has no dependency on the SNS service crate and no publish path at all. A client that wires an SNS topic (or an email subscription through it) to a Snow job and then waits for the state notification waits forever, and a workflow that is driven by those notifications never advances.

## Evidence

Spec/model grounding: `com.amazonaws.snowball#Notification` — "The Amazon Simple Notification Service (Amazon SNS) notification settings associated with a specific job … When the notification settings are defined during job creation, you can choose to notify based on a specific set of job states using the JobStatesToNotify array of strings, or you can specify that you want to have Amazon SNS notifications sent out for all job states with NotifyAll set to true"; `Notification$JobStatesToNotify` — "The list of job states that will trigger a notification for this job"; `Notification$NotifyAll` — "Any change in job state will trigger a notification for this job"; `Notification$SnsTopicARN` — "The new SNS TopicArn that you want to associate with this job … You can subscribe email addresses to an Amazon SNS topic". This is also well-established Snow Family behavior (job status-change notifications are delivered through the SNS topic attached at job creation).

Handler evidence: `src/ops/create_job.rs` (handle) stores `notification: input.notification` into `crate::state::StoredJob` and returns; `src/state.rs` `StoredJob.notification` / `to_job_metadata()` only echo it back on `DescribeJob`; `src/ops/cancel_job.rs` sets `job.job_state = crate::state::JobState::Cancelled` for the job (and its job parts) with no notification side effect. `grep -i sns` over the crate finds only the `Notification` field names in `src/ops/types/shared_types.rs`, and `Cargo.toml` declares no dependency on `service-com-amazonaws-sns-amazonsimplenotificationservice` (other crates in this repo do wire cross-service delivery this way, e.g. batch → eventbridge/sqs, dynamodb → dynamodbstreams).

Test coverage: none. `tests/all/job_lifecycle.rs` and `tests/all/job_updates.rs` only assert the `Notification` structure round-trips through `DescribeJob`; no test asserts any message is delivered to the configured topic.

## Steps to Reproduce

1. Create an SNS topic on the co-hosted SNS emulator and subscribe something observable to it (e.g. an SQS queue), noting the topic ARN.
2. CreateAddress with a deliverable address and keep the returned AddressId.
3. CreateJob with { JobType: IMPORT, AddressId: <id>, SnowballType: EDGE, ShippingOption: SECOND_DAY, Notification: { SnsTopicARN: <topic arn>, JobStatesToNotify: ["New", "Cancelled"], NotifyAll: false } }.
4. Poll the subscription: expected a job-state notification for the job entering `New`; observed nothing is ever delivered.
5. CancelJob with the returned JobId (the job is still in `New`, so the cancel succeeds and DescribeJob reports JobState `Cancelled`).
6. Poll the subscription again: expected a notification for the `Cancelled` transition; observed nothing. Repeat with NotifyAll=true and no JobStatesToNotify — still nothing is published.
