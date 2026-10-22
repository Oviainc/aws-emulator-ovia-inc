Issue-Source: trace-sweep-divergence
Issue-Trace: logs-log-group-configuration-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-log-group-configuration-lifecycle logs-log-group-lifecycle-and-tags logs-log-group-stream-data-roundtrip
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteLogGroup|4xx->2xx|$
Issue-Key: logs|DeleteLogGroup|4xx->2xx|status
Issue-Key: logs|DeleteLogGroup|2xx->4xx|$
Issue-Key: logs|DeleteLogGroup|2xx->4xx|status
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*]
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# DeleteLogGroup ignores deletionProtectionEnabled and desynchronises log-group state

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLogGroup

## Description
The emulator does not honour a log group's deletion-protection flag: DeleteLogGroup succeeds on a protected log group where the recorded service refuses it. Because the group is then actually gone, the rest of each fixture's lifecycle diverges -- the later delete that AWS accepts now 404s. A fourth cause shows the inverse state-tracking defect on log streams, where a stream the service no longer lists is still returned. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4 -- DeleteLogGroup succeeds on a log group with deletion protection enabled.
AWS: Smithy -- LogGroup.deletionProtectionEnabled targets DeletionProtectionEnabled (boolean), documented as "Indicates whether deletion protection is enabled for this log group. When enabled, deletion protection blocks all deletion operations until it is explicitly disabled." DeleteLogGroup models InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException and ValidationException, all client errors at 400. Recorded: deleting a protected log group returns 400 with __type InvalidParameterException and a message stating deletion protection must be disabled first.
Emulator: 200 with an empty body -- the protected log group is deleted.
Why: deletion protection is the guardrail that exists precisely to make an accidental delete fail. The emulator deletes the resource the real service would have preserved, so a client testing that its guardrail holds gets a false pass and the data it was protecting is gone.
Fix: the DeleteLogGroup handler -- check the log group's deletionProtectionEnabled state before deleting and refuse with the modeled InvalidParameterException at 400.
Traces: logs-log-group-configuration-lifecycle#task#4 (also logs-log-group-lifecycle-and-tags#task#15).

Cause 2 of 4 -- the subsequent legitimate DeleteLogGroup 404s because the group was already wrongly deleted.
AWS: as cause 1. Recorded: later in the same fixture, after deletion protection has been disabled, DeleteLogGroup answers 200 with an empty body.
Emulator: 400 with __type ResourceNotFoundException -- the log group no longer exists, because cause 1 deleted it.
Why: downstream of cause 1 but independently client-visible: the emulator reports a resource as absent that the service still holds, so a disable-protection-then-delete sequence (the documented way to remove a protected log group) fails at the delete. A fixer must re-run this step after fixing cause 1 to confirm the lifecycle completes.
Fix: the DeleteLogGroup handler -- follows from cause 1; verify the protected group still exists at this point.
Traces: logs-log-group-configuration-lifecycle#task#12.

Cause 3 of 4 -- fixture teardown's DeleteLogGroup 404s for the same reason.
AWS: as cause 1. Recorded: the teardown delete answers 200.
Emulator: 400.
Why: as cause 2, on a second fixture -- confirming the deletion-protection defect desynchronises state beyond a single scenario, and that teardown cannot reliably clean up.
Fix: the DeleteLogGroup handler -- follows from cause 1.
Traces: logs-log-group-lifecycle-and-tags#teardown#17.

Cause 4 of 4 -- DescribeLogStreams still lists a log stream the service no longer returns.
AWS: Smithy -- DescribeLogStreams output logStreams: LogStreams; LogStream members include logStreamName, creationTime, firstEventTimestamp, lastEventTimestamp, lastIngestionTime, uploadSequenceToken, arn and the @deprecated storedBytes. Recorded: at this point in the roundtrip fixture the service returns no log streams for the log group.
Emulator: returns one fully populated LogStream.
Why: a stream removal is not reflected in the emulator's state, so a client that deletes a stream and verifies it is gone sees it still listed -- and a recreate-after-delete flow hits a spurious already-exists or reads stale event metadata. Hypothesis: the stream-removal path in this fixture does not mutate the stored stream set, the same class of state defect as causes 1-3 on log groups.
Fix: the log-stream state behind DescribeLogStreams -- the stream-removal path must evict the stream from the log group's stream set.
Traces: logs-log-group-stream-data-roundtrip#task#10.

## Evidence
Cause 1 (logs-log-group-configuration-lifecycle#task#4, 2 affected steps): status expected 400, got 200; body expected {'__type': 'InvalidParameterException', 'message': 'LogGroup has delete protection enabled. Disable delete protection before attempting to delete the resource.'}, got {} (empty body). The error code InvalidParameterException and the 400 status are the requirement; the quoted message is an example.
Cause 2 (logs-log-group-configuration-lifecycle#task#12): status expected 200, got 400; body expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'The specified log group does not exist'}.
Cause 3 (logs-log-group-lifecycle-and-tags#teardown#17): status expected 200, got 400.
Cause 4 (logs-log-group-stream-data-roundtrip#task#10, 200 on both sides): $.logStreams[0] expected None, got a populated LogStream (logStreamName, creationTime, firstEventTimestamp, lastEventTimestamp, lastIngestionTime, uploadSequenceToken, arn, storedBytes 0).
Smithy citations: LogGroup.deletionProtectionEnabled -> DeletionProtectionEnabled (boolean), documented to block all deletion operations until explicitly disabled; DeleteLogGroup errors [InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException, ValidationException]; DescribeLogStreams output logStreams -> LogStreams.
state_reset_failures for this replay was 0, so these are not cross-fixture state bleed.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Enable deletion protection on a log group, then replay logs-log-group-configuration-lifecycle step task#4 (DeleteLogGroup on the protected group): the emulator answers 200 with an empty body; the recording answers 400 InvalidParameterException.
3. Continue the same fixture to task#12 (DeleteLogGroup after protection is disabled): the emulator answers 400 ResourceNotFoundException; the recording answers 200.
4. Replay logs-log-group-lifecycle-and-tags#teardown#17 (DeleteLogGroup in teardown): the emulator answers 400; the recording answers 200.
5. Replay logs-log-group-stream-data-roundtrip#task#10 (DescribeLogStreams): the emulator returns a log stream the recording omits entirely.
