Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__sqs__ssm.instance-job-writes-objects-and-every-s3-event-lands-in-sqs-with-matching-etag-size-sequencer ec2__s3__sqs__ssm.sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer ec2__s3__sqs__ssm.sqsrace-two-workers-one-queue-write-every-job-result-exactly-once
Issue-Key: ec2__s3__sqs__ssm.instance-job-writes-objects-and-every-s3-event-lands-in-sqs-with-matching-etag-size-sequencer|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__s3__sqs__ssm.sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer|_retry_dependency#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__s3__sqs__ssm.sqsrace-two-workers-one-queue-write-every-job-result-exactly-once|_retry_dependency#7|Timeout|time.sleep(interval_s)
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# ec2__s3__sqs__ssm: an EC2 instance never runs its job, so no object is written and no S3 event reaches SQS

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: an instance launched with a user-data (or SSM-driven) job never executes it, so the whole producer side of these pipelines is silent.
AWS: recorded behavior only — RunInstances with user data that writes objects to S3 brings the instance to 'running', the guest job writes one object per work item, and each write raises an s3:ObjectCreated notification that lands in the configured SQS queue with ETag, size and sequencer matching the object; a second worker draining the same queue writes every job result exactly once.
Emulator: nothing the guest was asked to do happens — no object is written, so no notification is produced and the queue stays empty; each check polls for its condition and then fails with 'Check timed out after 60.0s'.
Why: any workload whose compute runs inside an EC2 instance (and anything downstream of the objects it writes — S3 event notifications, SQS fan-out, exactly-once worker semantics) is unobservable, and a client waits out its entire budget with no signal that the instance is inert.
Fix: the EC2 instance runtime in the ec2 service crate — whatever must execute an instance's user data / SSM command document and let its guest-side AWS calls reach the emulator.
Traces: ec2__s3__sqs__ssm.instance-job-writes-objects-and-every-s3-event-lands-in-sqs-with-matching-etag-size-sequencer, ec2__s3__sqs__ssm.sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer, ec2__s3__sqs__ssm.sqsrace-two-workers-one-queue-write-every-job-result-exactly-once

## Evidence
Replay detail for all three checks: 'Error: Check timed out after 60.0s'. Expected: the guest job's objects present in the bucket and one s3:ObjectCreated message per object in the queue (ETag/size/sequencer matching), within the check's polling budget. Actual: the polled condition is never reached; bucket and queue remain empty.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay the three ec2__s3__sqs__ssm checks in trace_ids. 3. Directly: create a bucket with an SQS notification configuration, RunInstances with user data that puts objects into it, then poll the bucket and ReceiveMessage on the queue — both stay empty.
