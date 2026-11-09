Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-train-host-and-invoke-xgboost-model
Issue-Step: teardown#18
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-train-host-and-invoke-xgboost-model
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: logs|DescribeLogStreams|2xx->4xx|status
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# CloudWatch Logs DescribeLogStreams refuses an existing log group with 400

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogStreams

## Description
1 cause, in the DescribeLogStreams handler's log-group resolution.

Cause 1 of 1 - DescribeLogStreams answers HTTP 400 for a log group AWS describes successfully.
AWS: Smithy: DescribeLogStreams takes logGroupName (@length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$) or logGroupIdentifier and returns logGroups' streams as output member logStreams -> LogStreams plus an optional nextToken; it is @paginated (inputToken nextToken, outputToken nextToken, items logStreams, pageSize limit). Its modeled client failures are ResourceNotFoundException (400-class) when the named log group does not exist and InvalidParameterException (400-class) for a malformed request - neither applies to a log group that exists. For the step recorded here the group is the one a SageMaker training job writes to (the /aws/sagemaker/... group created on the caller's behalf when the job ran earlier in the same fixture), and the recording shows HTTP 200 with the group's stream list - including the empty-list case, which AWS returns as a 200 with an empty logStreams rather than an error. Beyond the status the recording asserts nothing further, so: recorded behavior only for the body.
Emulator: answers HTTP 400 for that same group. Hypothesis: the emulator never materialises the log group a training job writes to, so DescribeLogStreams resolves nothing and falls into its not-found branch - which is consistent with the DeleteLogGroup step immediately before it (teardown#17) failing the same way on the same fixture.
Why: a client that reads a job's logs - the normal way to diagnose a training job, and what every SDK/CLI 'tail the logs' path does - cannot enumerate the streams at all, and gets a client error indistinguishable from 'you asked for a group that does not exist'. A teardown or log-collection routine that lists streams before deleting them aborts, or must treat every 400 as success, at which point a genuinely malformed request is swallowed too.
Fix: the DescribeLogStreams handler's log-group resolution together with the logs state that should hold the group implicitly created for a SageMaker training job (an existing group must resolve and return 200 with its streams, empty list included); 400 ResourceNotFoundException must be reserved for a group that genuinely does not exist.
Traces: sagemaker-train-host-and-invoke-xgboost-model#teardown#18.

Related but not covering: 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md owns the DeleteLogGroup cluster (and is keyed to a lambda fixture). It is a different operation and the replay reports this DescribeLogStreams cluster as unowned, so it is filed with its own replay key; the two are plausibly the same underlying log-group-resolution gap and should be verified together.

## Evidence
sagemaker-train-host-and-invoke-xgboost-model#teardown#18 DescribeLogStreams - "status: expected 200, got 400"; divergence_key service=logs|DescribeLogStreams|status=200->400|keys=status. The divergence is on the status alone: the recording has 200 for the log group the fixture's training job wrote to earlier in the same run, and the emulator answers 400. The immediately preceding step teardown#17 (DeleteLogGroup, divergence_key service=logs|DeleteLogGroup|status=200->400|keys=status) fails identically on the same group, which is the basis for the shared-resolution hypothesis above.

## Steps to Reproduce
1. Replay fixture sagemaker-train-host-and-invoke-xgboost-model through to its teardown, so the training job has run and its log group exists.
2. At teardown#18, call DescribeLogStreams with the training job's logGroupName.
3. Observe HTTP 400 where the recording has HTTP 200 with the group's logStreams (an empty list is a valid 200).
