Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__pipes__sqs.kinesis-source-dead-letter-config-requires-sqs-or-sns kinesis__pipes__sqs.kinesis-source-sqs-target-delivers-data-base64-and-partition-key
Issue-Key: kinesis__pipes__sqs.kinesis-source-dead-letter-config-requires-sqs-or-sns|run#25|AssertionError|assert terminal["CurrentState"] == "CREATE_FAILED", (
Issue-Key: kinesis__pipes__sqs.kinesis-source-sqs-target-delivers-data-base64-and-partition-key|run#20|TimeoutError|raise TimeoutError("every record reaching the SQS target: the run budget ended first; not graded")
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# kinesis__pipes__sqs: 2 net-new causes — a Kinesis DeadLetterConfig target is accepted instead of failing into CREATE_FAILED, and a Kinesis-source pipe delivers nothing to its SQS target

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two distinct defects on the Kinesis-source path. Neither replay key is carried by an open issue.

Cause 1 of 2: an invalid DeadLetterConfig destination is accepted and the pipe runs.
AWS: SourceParameters.KinesisStreamParameters.DeadLetterConfig.Arn must name an SQS queue or an SNS topic. An ARN naming a Kinesis stream is not refused synchronously — CreatePipe returns 200 — but the pipe then fails asynchronously: it settles in CurrentState=CREATE_FAILED with a StateReason naming the unsupported dead-letter destination, and never reaches RUNNING.
Emulator: the same pipe settles in CurrentState='RUNNING' with StateReason='USER_INITIATED'. The unsupported DLQ destination is never validated at any point in the lifecycle.
Why: a misconfigured pipe is reported healthy, so the client never learns its dead letters have nowhere to go; a negative test for this misconfiguration passes against the emulator and the fault only appears in AWS. It also means the emulator has no asynchronous CREATE_FAILED path at all for a pipe whose configuration is only checkable at activation.
Fix: the Pipes pipe-activation path (the state machine that advances CREATING -> RUNNING) must validate DeadLetterConfig.Arn's service namespace (sqs|sns) and settle the pipe in CREATE_FAILED with a StateReason instead of RUNNING/USER_INITIATED.
Traces: kinesis__pipes__sqs.kinesis-source-dead-letter-config-requires-sqs-or-sns.

Cause 2 of 2: a RUNNING Kinesis-source pipe never delivers to its SQS target.
AWS: a RUNNING pipe with a Kinesis source delivers each record to the SQS target as the documented Kinesis record envelope — kinesisSchemaVersion, eventID, eventSource "aws:kinesis", eventSourceARN, awsRegion, and a kinesis object whose `data` is the base64 of the record's Data and whose `partitionKey` is the record's partition key. Recorded behavior shows every PutRecords record arriving.
Emulator: no record ever reaches the SQS target; the check exhausts its run budget waiting (TimeoutError "every record reaching the SQS target: the run budget ended first").
Why: a Kinesis->Pipes->SQS flow loses 100% of its traffic while the pipe reports RUNNING, and because the wait never completes the failure costs the whole run budget instead of failing fast. The base64 `data` and `partitionKey` envelope fidelity is also entirely unobservable.
Fix: the Pipes runtime's Kinesis source poller and the SQS target stage's Kinesis record-envelope serializer. Distinct from the SQS-source non-delivery causes in 20261107T175408Z-pipes-3-net-new-causes-a-running-pipe.md.
Traces: kinesis__pipes__sqs.kinesis-source-sqs-target-delivers-data-base64-and-partition-key.

## Evidence
Cause 1 — AssertionError: a Kinesis-source pipe whose DeadLetterConfig.Arn names a Kinesis stream must fail asynchronously into CREATE_FAILED, got CurrentState='RUNNING' (StateReason='USER_INITIATED').

Cause 2 — TimeoutError: every record reaching the SQS target: the run budget ended first; not graded. No record was delivered from the Kinesis source to the SQS target.

## Steps to Reproduce
1. CreatePipe with a Kinesis source, an SQS target and SourceParameters.KinesisStreamParameters.DeadLetterConfig.Arn set to a Kinesis stream ARN; poll DescribePipe until terminal. Emulator: RUNNING / USER_INITIATED; AWS: CREATE_FAILED.
2. CreatePipe with a Kinesis source and an SQS target (valid role, DesiredState=RUNNING), wait for RUNNING, PutRecords a few records, then ReceiveMessage on the target: nothing arrives, and each delivered record should carry kinesis.data (base64 of Data) and kinesis.partitionKey.
