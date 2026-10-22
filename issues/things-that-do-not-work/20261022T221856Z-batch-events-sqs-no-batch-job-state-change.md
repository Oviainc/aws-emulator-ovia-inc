Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# batch__events__sqs: no Batch Job State Change event is ever delivered to a matching EventBridge rule, so the cross-service job-event path cannot complete

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: the Batch -> EventBridge -> SQS job-state-change path never produces an event.

AWS: recorded behavior only (this check is marked unblessed/DRAFT in the suite, so its expectation comes from the EventBridge 'AWS Batch events' reference it cites rather than a replayed oracle): AWS Batch publishes job state change notifications to the DEFAULT EventBridge bus with source == 'aws.batch' and detail-type == 'Batch Job State Change' as a job moves through its states; a rule matching that source/detail-type delivers the event to its SQS target, and a rule that does not match must NOT receive it.

Emulator: the check is killed by the harness at 60s ('Error: Check timed out after 60.0s') with no event ever arriving on the subscribed queue. It cannot be separated from the Batch job-state-machine defect filed for service `batch` in this same sweep ('submitted jobs are driven straight to FAILED instead of resting in RUNNABLE'): a job whose status is fabricated terminal immediately, rather than transitioning, produces no observable state-change sequence to publish, so whether the emulator emits 'Batch Job State Change' at all is still unverified here. Hypothesis: the Batch service state performs no EventBridge publication on job state transitions.

Why: a client that reacts to job completion through EventBridge (the documented, polling-free way to observe Batch) receives nothing, and the negative half of the contract (an unrelated rule must not receive the event) is equally unobservable — so a consumer cannot be developed or tested against the emulator at all.

Fix: Batch job-state-transition publication into the default EventBridge bus (the Batch service state's transition hook, emitting source 'aws.batch' / detail-type 'Batch Job State Change'), gated behind the job state machine fix in the `batch` issue; re-replay this check after that lands to determine whether a second defect remains in the emission itself.

Traces: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue.

## Evidence
From the simparity-behavioral-aws-v1 replay: per-service line 'batch__events__sqs 0/1 (0%) 1 failed'.

#### [mismatch] batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Error: Check timed out after 60.0s

The check's own header records the expectation it is grounded in: 'H1 AWS Batch emits job state change events to the DEFAULT EventBridge bus with source == "aws.batch" and detail-type == "Batch Job State Change". Source: the EventBridge reference page "AWS Batch events" (events-ref-batch.html)'. Expected: the matching rule's SQS queue receives the event and the non-matching rule's queue does not. Actual: nothing is delivered within the check's budget; the run is terminated at 60s.

Corroborating, from the same replay: batch.op-contract-listJobs reports 'job ... must rest in RUNNABLE, got FAILED', i.e. Batch job status in this emulator does not transition, which is the upstream condition this event path depends on.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Create an SQS queue and an EventBridge rule on the default bus with event pattern {"source": ["aws.batch"], "detail-type": ["Batch Job State Change"]}, targeting that queue; create a second queue behind a rule whose pattern does not match.
3. Create a Batch compute environment + ENABLED job queue, register a container job definition, and aws batch submit-job onto the queue.
4. Poll aws sqs receive-message on the matching queue: no 'Batch Job State Change' message with source 'aws.batch' ever arrives (the behavioral check is killed at 60s), so neither the delivery nor the withholding half of the contract can be observed.
