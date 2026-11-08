Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconvert.job-canceled-event-delivered-via-eventbridge
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert.job-canceled-event-delivered-via-eventbridge|_poll_for_job_event#32|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: 57573ec352e72f33b5d5f16a1d219541db631512

# MediaConvert publishes no "MediaConvert Job State Change" event: pending_job_events is recorded but never drained onto the EventBridge bus

> _Note: this issue was identified against the code base at commit `57573ec352e72f33b5d5f16a1d219541db631512`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Cause 1 of 1: a MediaConvert job's transition out of SUBMITTED is recorded for publication but never published, so an EventBridge rule matching {"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]} with an SQS target never receives anything, and a client waiting on MediaConvert eventing waits forever. Failing check: mediaconvert.job-canceled-event-delivered-via-eventbridge.

NOTE: the open issue 20261107T231832Z-mediaconvert-emits-no-mediaconvert-job-state-change-event.md describes the same area but did NOT carry this replay key in the current report (the check was not reported as OWNED), so it does not cover this replay; it is named here so a fixer can close both together rather than twice.

AWS: the Smithy model for MediaConvert models no eventing at all — there is no PutEvents-like operation and no event shape bound to com.amazonaws.mediaconvert#MediaConvert (the operations the check drives are CreateJob POST /2017-08-29/jobs -> 201, CancelJob, GetJob, ListJobs, CreateQueue, DeleteQueue), so this is recorded/documented behavior only. AWS documents (https://docs.aws.amazon.com/mediaconvert/latest/ug/how-mediaconvert-jobs-progress.html) that once a job begins processing MediaConvert publishes its state changes to Amazon EventBridge as an event whose fixed text is source == "aws.mediaconvert" and detail-type == "MediaConvert Job State Change", with a detail object whose variable parts come from the job record: detail.jobId is the Id CreateJob minted for that job and detail.status is the JobStatus the job moved into (PROGRESSING / COMPLETE / ERROR). Only the two fixed strings and the presence/meaning of detail.jobId and detail.status are the requirement; the id, timestamps, account and region in any one recording merely fill that template. The contract has a negative half too: a job canceled while its queue was PAUSED — i.e. before it ever began processing — publishes NO Job State Change event at all.

Emulator: no event is ever delivered. The check creates an SQS queue, puts a rule with that pattern, puts the SQS target (FailedEntryCount 0), sets a queue policy allowing events.amazonaws.com sqs:SendMessage, creates a role trusting mediaconvert.amazonaws.com, submits a job to an ACTIVE queue and then drains the queue. The control-plane half is satisfied (the PAUSED-queue job reaches CANCELED via CancelJob/GetJob), but the SQS target stays empty: the check never reaches its assertions on the event body and is killed by the harness — "Check timed out after 180.0s" — inside the receive_message/delete_message poll loop. Code evidence in the current checkout is under Traces.

Why: a client using the documented event-driven MediaConvert workflow — submit a job, then react to its Job State Change events via an EventBridge rule onto SQS/SNS/Lambda — hangs indefinitely against the emulator instead of observing the job's terminal transition. Worse, because "no event" is indistinguishable from "no event yet", the negative half of the contract (a canceled-before-start job emitting nothing) cannot be verified either, so a test that asserts that silence passes vacuously here and is meaningless.

Fix: the mediaconvert crate's job-lifecycle state machine and its missing event publisher. Two parts both need to land: (a) a publisher that drains the recorded transitions onto the EventBridge bus as source=aws.mediaconvert / detail-type=MediaConvert Job State Change with detail.jobId and detail.status taken from the job record — the field that holds them is documented as being drained by a `publish_pending` that has no definition anywhere in the state module; and (b) a driver that advances the job clock without requiring a subsequent MediaConvert request, because the clock is lazy (the settle path runs only from a request handler that reads job state), so a client that submits a job and then only waits on EventBridge never even materialises the transition that would be published. The negative half must be preserved: a job held back by a paused queue, or taken out by CancelJob before it began, records and publishes nothing.

Traces: mediaconvert.job-canceled-event-delivered-via-eventbridge. Supporting citations, services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs: lines ~3712-3719 declare `pending_job_events: VecDeque<JobStateChange>` documented as "the job state changes this store has recorded and nothing has published to EventBridge yet" and "drained — with this store's lock released — by the publisher ([`publish_pending`])"; line ~3794 initialises it empty; lines ~3940-3942 state settle_job is "the one place a job's status MOVES on its own, so it is where the move is recorded for publication to EventBridge"; line ~3985 calls `self.record_job_state_change(id, due)`. A search of state.rs finds `publish_pending` ONLY inside that doc link — it is never defined — so nothing drains the queue onto the bus.

## Evidence
Check: mediaconvert.job-canceled-event-delivered-via-eventbridge [mismatch]. Suite simparity-behavioral-aws-v1, mediaconvert 20/23 (87%).

Expected (AWS, documented behavior): after CreateJob on an ACTIVE queue, a message arrives on the SQS target of a rule whose pattern is {"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]}, whose JSON body has source == "aws.mediaconvert", detail.jobId == the created job's Id and detail.status == the JobStatus the job moved into (ERROR in this check's control). And: continuing to drain for a further window must show NO event at all for the job that was created on a PAUSED queue and then canceled.

Actual (emulator): no message is ever delivered for the job created on the ACTIVE queue. The check's first half passes (the canceled job is observed at Status 'CANCELED' through GetJob), then the poll loop (sqs.receive_message MaxNumberOfMessages=10, WaitTimeSeconds=10, timeout_s=900) never sees a message, and the harness terminates the check: "Error: Check timed out after 180.0s". No assertion on an event body is ever reached, so neither the positive event nor the required silence is observable.

No status/error-code divergence is involved: every API call in the check succeeds; the divergence is the absent asynchronous publication.

Code evidence (current checkout): services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs ~3712-3719 (`pending_job_events`, doc naming `publish_pending` as its drainer), ~3794 (init), ~3940-3942 and ~3985 (`record_job_state_change` called from `settle_job`). `publish_pending` occurs in the file only as that doc link and has no definition.

## Steps to Reproduce
1. Boot the emulator and point botocore mediaconvert, events, sqs and iam clients at it (region us-west-2).
2. sqs.create_queue(QueueName=Q); read QueueArn via get_queue_attributes.
3. events.put_rule(Name=R, EventPattern='{"source":["aws.mediaconvert"],"detail-type":["MediaConvert Job State Change"]}'); events.put_targets(Rule=R, Targets=[{"Id":"sqs-target","Arn":<QueueArn>}]) and assert FailedEntryCount == 0.
4. sqs.set_queue_attributes(QueueUrl=..., Attributes={"Policy": <allow events.amazonaws.com sqs:SendMessage on that queue, Condition ArnEquals aws:SourceArn = the rule arn>}).
5. iam.create_role with a trust policy allowing sts:AssumeRole to Principal.Service "mediaconvert.amazonaws.com"; mc.create_queue(Name=QA) (ACTIVE).
6. mc.create_job(Role=<that role arn>, Queue=QA, Settings={"Inputs":[{"FileInput":"s3://b/in.mp4"}],"OutputGroups":[{"Name":"File Group","OutputGroupSettings":{"Type":"FILE_GROUP_SETTINGS","FileGroupSettings":{"Destination":"s3://b/out/"}},"Outputs":[{"ContainerSettings":{"Container":"MP4"},"VideoDescription":{"CodecSettings":{"Codec":"H_264","H264Settings":{"RateControlMode":"QVBR","QvbrSettings":{"QvbrQualityLevel":7},"MaxBitrate":5000000,"SceneChangeDetect":"ENABLED"}}}}]}]}).
7. Without issuing any further MediaConvert call, poll sqs.receive_message(QueueUrl=..., MaxNumberOfMessages=10, WaitTimeSeconds=10) in a loop. Expect a message whose body has source 'aws.mediaconvert', detail-type 'MediaConvert Job State Change', detail.jobId == the job's Id and detail.status == its terminal status; observe the queue stays empty indefinitely.
8. Negative half (only verifiable once step 7 works): create a PAUSED queue, submit a job to it, mc.cancel_job(Id=...), confirm GetJob reports CANCELED, and confirm that draining the same SQS queue for a further window shows no event carrying that job's id.
