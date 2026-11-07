Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconvert.job-canceled-event-delivered-via-eventbridge
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert.job-canceled-event-delivered-via-eventbridge|_poll_for_job_event#38|Timeout|return {**seen, "event": body}
Issue-Commit: 980a4c09fd08f28bfbc7e3936204008444675b82

# MediaConvert emits no "MediaConvert Job State Change" event to EventBridge: a job's terminal transition is never published, so an EventBridge rule targeting SQS never fires

> _Note: this issue was identified against the code base at commit `980a4c09fd08f28bfbc7e3936204008444675b82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Cause 1 of 1: a MediaConvert job that leaves SUBMITTED never produces an EventBridge event, so nothing downstream of an `aws.mediaconvert` / `MediaConvert Job State Change` rule ever sees the job's state changes. Failing check: mediaconvert.job-canceled-event-delivered-via-eventbridge.

AWS: the Smithy model for MediaConvert models no eventing at all (there is no PutEvents-like operation and no event shape) — so this is recorded/documented behavior only. AWS documents that MediaConvert publishes a job's state changes to Amazon EventBridge once the job begins processing (https://docs.aws.amazon.com/mediaconvert/latest/ug/how-mediaconvert-jobs-progress.html): an event with `"source": "aws.mediaconvert"`, `"detail-type": "MediaConvert Job State Change"`, and a `detail` object carrying at least `jobId` (the id CreateJob minted for that job) and `status` (the JobStatus the job moved into — PROGRESSING / COMPLETE / ERROR). The values are taken from the job and its account/region; only the shape and the source/detail-type strings are the requirement. Conversely, a job that is canceled before it ever begins processing publishes NO Job State Change event — the absence is also part of the contract.

Emulator: the job's own control plane does move (the check's first half, the PAUSED-queue job reaching CANCELED via CancelJob/GetJob, is satisfied), and the job created on the ACTIVE queue does reach a terminal status (ERROR — see the related open issue 20261024T051247Z-mediaconvert-getjob-wrong-behavior.md, which is about the status itself, not about eventing). But no event is ever delivered: the check wires an EventBridge rule (`put_rule` with pattern {"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]}) to an SQS target with a queue policy allowing events.amazonaws.com, then drains the queue; no message for the job's id ever arrives and the check is killed by the harness timeout ("Check timed out after 180.0s") while polling, rather than failing an assertion on a delivered event's content.

Two things in the state layer look implicated and should both be checked by the fixer: (a) `settle_job` records the transition for publication via `record_job_state_change` into `pending_job_events`, and the field's own doc (services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs around line 3646-3653) says it is "drained — with this store's lock released — by the publisher ([`publish_pending`])", but a search of state.rs finds `publish_pending` only in that doc link and never as a definition, i.e. nothing appears to drain the queue onto the EventBridge bus; (b) the whole job clock is lazy — `ComAmazonawsMediaconvertMediaconvertState::settle` runs only on a request path that reads job state, so a client that submits a job and then only waits on EventBridge (never calling GetJob/ListJobs again, which is the normal event-driven workflow) never even materialises the transition that would be recorded for publication.

Why: a client that uses the documented event-driven MediaConvert workflow — submit a job, then react to its Job State Change events on an EventBridge rule (Lambda/SQS/SNS target) — hangs forever against the emulator. It observes no COMPLETE/ERROR event and no progress, so pipelines built on MediaConvert eventing cannot be exercised at all; and because the absence is indistinguishable from "no event yet", a test asserting that a canceled-before-start job emits nothing also cannot be trusted.

Fix: the job-lifecycle state machine and its event publisher in the mediaconvert crate — the code that records a job's exit from SUBMITTED (`settle_job` / `record_job_state_change` / `pending_job_events` in src/state.rs) needs an actual publisher that drains those records onto the EventBridge bus as `source=aws.mediaconvert`, `detail-type=MediaConvert Job State Change`, with `detail.jobId` and `detail.status` from the job record, plus a driver that advances the job clock without requiring a subsequent MediaConvert request so the transition (and therefore its event) happens on its own.

Traces: mediaconvert.job-canceled-event-delivered-via-eventbridge (harness timeout while polling the SQS target of an `aws.mediaconvert` rule).

## Evidence
Expected (AWS): after CreateJob on an ACTIVE queue, an event is delivered through a rule matching {"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]} to its SQS target, whose body has source == "aws.mediaconvert", detail.jobId == the created job's Id and detail.status == the terminal JobStatus; and no such event is delivered for a job canceled while its queue was PAUSED.

Actual (emulator): the check's control-plane half passes (the canceled job reaches CANCELED via GetJob), but the SQS target receives no message at all for the job created on the ACTIVE queue. The check does not reach its assertions on the event body; it is terminated by the harness: "Check timed out after 180.0s" while inside _poll_for_job_event (receive_message/delete_message loop with WaitTimeSeconds=10).

Supporting code evidence in the checkout: services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs lines ~3646-3653 declare `pending_job_events: VecDeque<JobStateChange>` documented as "The job state changes this store has recorded and nothing has published to EventBridge yet" and "drained … by the publisher ([`publish_pending`])"; lines ~3915-3920 call `self.record_job_state_change(id, due)` when a job moves. `publish_pending` appears in state.rs only as that doc link, with no definition in the file, and `settle` (line ~3774) is driven only from request handlers.

## Steps to Reproduce
1. Boot the emulator and point mediaconvert, events, sqs and iam clients at it.
2. sqs.create_queue; read its QueueArn; events.put_rule(Name=r, EventPattern={"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]}); events.put_targets(Rule=r, Targets=[{Id:'sqs-target', Arn:<queue arn>}]); sqs.set_queue_attributes with a Policy allowing events.amazonaws.com sqs:SendMessage on that queue.
3. iam.create_role with a trust policy for mediaconvert.amazonaws.com; mc.create_queue(Name=q) (ACTIVE).
4. mc.create_job(Role=<role arn>, Queue=q, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}],'OutputGroups':[{...FILE_GROUP_SETTINGS...}]}).
5. Without calling any further MediaConvert operation, poll sqs.receive_message(QueueUrl=..., MaxNumberOfMessages=10, WaitTimeSeconds=10) in a loop.
6. Expect a message whose JSON body has source 'aws.mediaconvert', detail-type 'MediaConvert Job State Change', detail.jobId == the job's Id and detail.status set to the job's terminal status; observe that the queue stays empty indefinitely (the check is killed at the 180s harness timeout).
7. Separately: a job created on a PAUSED queue and then canceled must emit no such event — that half cannot be verified while no event is ever emitted.
