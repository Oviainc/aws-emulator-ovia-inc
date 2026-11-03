Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records
Issue-Step: teardown#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: logs|DeleteLogGroup|2xx->4xx|status
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# CloudWatch Logs DeleteLogGroup refuses an existing log group

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLogGroup

## Description
1 cause, in the DeleteLogGroup handler.

Cause 1 of 1 - DeleteLogGroup answers HTTP 400 for a log group AWS deletes successfully.
AWS: recorded behavior only for the status - the recording shows DeleteLogGroup answering HTTP 200 with an empty body for the Lambda function's log group (/aws/lambda/<function-name>), which is what the operation does for an existing group; its modelled failure classes are a 400-class ResourceNotFoundException when the group does not exist and InvalidParameterException for a malformed name, neither of which applies to a group that was created earlier in the same fixture.
Emulator: answers HTTP 400 for that same existing log group.
Why: a teardown or cleanup routine that deletes the log groups it created fails on a group it just created, so the caller either aborts its cleanup or has to treat every 400 as success - at which case a genuinely malformed request is also swallowed. Any client that distinguishes 'deleted' from 'invalid request' cannot do so here.
Fix: the DeleteLogGroup handler's log-group resolution (the group created implicitly for a Lambda function must be deletable by name).
Traces: lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records#teardown#12.

## Evidence
lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records#teardown#12 - DeleteLogGroup: status expected 200, got 400. The log group is the one implicitly created for the fixture's Lambda function earlier in the same run; the divergence is on the status alone (divergence_key service=logs|DeleteLogGroup|status=200->400|keys=status).

## Steps to Reproduce
1. Replay fixture lambda-onyx-traj-alpha-is-a-stage-not-an-alias-then-replay-the-records through to its teardown.
2. Observe teardown#12's DeleteLogGroup against /aws/lambda/<function-name>: the emulator answers 400 where the recording has 200.
