Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-error-conflict-and-source-notfound
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-error-conflict-and-source-notfound pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-pipe-error-paths pipes-filter-criteria-nested-json-preserved pipes-sqs-to-sqs-lifecycle pipes-pipe-lifecycle-immediate-conflicts pipes-pipe-tag-management pipes-sqs-source-eventbus-target pipes-update-pipe-config pipes-update-pipe-workflow pipes-validation-and-conflict-errors
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|DescribePipe|2xx->2xx|$.CreationTime
Issue-Key: pipes|DescribePipe|2xx->2xx|$.EnrichmentParameters
Issue-Key: pipes|DescribePipe|2xx->2xx|$.LastModifiedTime
Issue-Key: pipes|DescribePipe|2xx->2xx|$.SourceParameters
Issue-Key: pipes|DescribePipe|2xx->2xx|$.StateReason
Issue-Key: pipes|DescribePipe|2xx->2xx|$.Tags
Issue-Key: pipes|DescribePipe|2xx->2xx|$.TargetParameters
Issue-Key: pipes|DescribePipe|2xx->2xx|$.SourceParameters.SqsQueueParameters
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn
Issue-Key: pipes|DeletePipe|2xx->2xx|$.CreationTime
Issue-Key: pipes|DeletePipe|2xx->2xx|$.DesiredState
Issue-Key: pipes|DeletePipe|2xx->2xx|$.LastModifiedTime
Issue-Commit: 533efece7fa11c0e537273065624bf5d01aa80f8

# pipes responses that describe a pipe drop modeled members and mis-serialize timestamps and DesiredState

> _Note: this issue was identified against the code base at commit `533efece7fa11c0e537273065624bf5d01aa80f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipe

## Description
This bundle has 8 causes. Every one is a 200 response whose BODY diverges: members the model defines and AWS populates are absent (serialized as None/omitted), timestamps are emitted in the wrong representation, and DeletePipe reports the wrong DesiredState. The causes sit in three different handlers (DescribePipe, ListPipes, DeletePipe) and in whatever state record they read, so a fixer must close all 8 before submitting.

Shared fault: the stored pipe record is not round-tripped. Configuration sub-structures (SourceParameters, TargetParameters, EnrichmentParameters), Tags, and StateReason are never written back onto the response, and timestamp members are serialized as a bare epoch-seconds float rather than the date-time representation the recordings assert.

Cause 1 of 8: DescribePipe omits SourceParameters, TargetParameters, EnrichmentParameters, Tags, StateReason and mis-serializes CreationTime/LastModifiedTime.
  AWS: DescribePipe (Smithy: GET /v1/pipes/{Name}, 200, output DescribePipeResponse) returns SourceParameters (PipeSourceParameters — for an SQS source, SqsQueueParameters with the pipe's BatchSize), TargetParameters (PipeTargetParameters), EnrichmentParameters (PipeEnrichmentParameters), Tags (TagMap) and StateReason (PipeStateReason, "The reason the pipe is in its current state."); for a pipe created and left in its requested state the recorded StateReason is the fixed token "USER_INITIATED". CreationTime/LastModifiedTime are Timestamp members and the model documents LastModifiedTime as "in ISO-8601 format (YYYY-MM-DDThh:mm:ss.sTZD)"; the recording asserts them by shape, which a bare epoch float does not satisfy.
  Emulator: those five members come back absent (None) — including SourceParameters, which the request supplied — and the timestamps come back as a bare number (e.g. 1793344196.198).
  Why: a client reading back the pipe it just created cannot see its own source/target/enrichment configuration, its tags, or why the pipe is in its state; any config-diff, idempotent-apply or CloudFormation-style read loop sees an empty pipe and rewrites it. A timestamp typed as a number where a date-time string is expected fails strict SDK deserialization and any ordering/formatting on creation time.
  Fix: DescribePipe's handler/serializer (services/service-com-amazonaws-pipes-pipes/src/ops/describe_pipe.rs) must project every stored member of the pipe record, and the service's timestamp serialization must use the modeled date-time representation.
  Traces: pipes-error-conflict-and-source-notfound#task#11 (also pipes-list-pipes-filtering#task#12/#14, pipes-list-pipes-pagination#task#8/#10/#12, pipes-pipe-error-paths#task#8).

Cause 2 of 8: DescribePipe on a pipe whose source carries nested FilterCriteria JSON loses SourceParameters.SqsQueueParameters, EnrichmentParameters, TargetParameters, Tags, StateReason and mis-serializes both timestamps.
  AWS: as cause 1; the nested FilterCriteria pattern is preserved inside SourceParameters and the sibling SqsQueueParameters (BatchSize) stays populated.
  Emulator: SourceParameters.SqsQueueParameters absent, plus the same omissions and epoch-float timestamps.
  Why: a pipe configured with a filter reads back without the batching/source settings it was created with.
  Fix: same component as cause 1 (projection of the nested source-parameter sub-structure).
  Traces: pipes-filter-criteria-nested-json-preserved#task#8.

Cause 3 of 8: ListPipes entries omit StateReason.
  AWS: ListPipes (Smithy: GET /v1/pipes, 200, output Pipes: PipeList of Pipe) returns Pipe.StateReason ("The reason the pipe is in its current state."); recorded value for a user-driven state is "USER_INITIATED".
  Emulator: the listed Pipe summary has StateReason absent.
  Why: a client listing pipes to find failed/odd ones has no state reason at all and must call DescribePipe per pipe (which, per cause 1, also omits it).
  Fix: ListPipes' summary projection (src/ops/list_pipes.rs) must carry StateReason from the stored record.
  Traces: pipes-list-pipes-filtering#task#15 (also pipes-list-pipes-filtering#task#16, pipes-list-pipes-pagination#task#13/#14/#15, pipes-sqs-to-sqs-lifecycle#task#11).

Cause 4 of 8: DescribePipe after a start/stop lifecycle call omits EnrichmentParameters, TargetParameters, Tags, StateReason.
  AWS: as cause 1 — the members persist across state transitions.
  Emulator: all four absent once the pipe has been started/stopped.
  Why: the pipe appears to lose its configuration and its state reason as a side effect of a lifecycle call.
  Fix: same component as cause 1.
  Traces: pipes-pipe-lifecycle-immediate-conflicts#task#10 (also pipes-sqs-to-sqs-lifecycle#task#10).

Cause 5 of 8: DescribePipe during tag management omits EnrichmentParameters, SourceParameters, TargetParameters, StateReason.
  AWS: as cause 1 — SourceParameters.SqsQueueParameters (BatchSize) and StateReason "USER_INITIATED" are present.
  Emulator: all four absent.
  Why: a tagging workflow that reads the pipe back cannot verify configuration alongside tags.
  Fix: same component as cause 1.
  Traces: pipes-pipe-tag-management#task#8.

Cause 6 of 8: DescribePipe for a pipe with an SQS source and an EventBus target omits EnrichmentParameters, SourceParameters, Tags, StateReason.
  AWS: as cause 1.
  Emulator: all four absent.
  Why: cross-service pipe configuration is unreadable after creation.
  Fix: same component as cause 1.
  Traces: pipes-sqs-source-eventbus-target#task#7.

Cause 7 of 8: DescribePipe after UpdatePipe omits EnrichmentParameters, SourceParameters, TargetParameters, Tags, StateReason.
  AWS: as cause 1 — after a successful update the response reflects the merged configuration.
  Emulator: all five absent, so an update cannot be confirmed.
  Why: a read-after-update client cannot tell whether its update applied.
  Fix: same component as cause 1, plus the update path's write-back into state (src/ops/update_pipe.rs / src/state.rs).
  Traces: pipes-update-pipe-config#task#10 (also pipes-update-pipe-workflow#task#9, pipes-validation-and-conflict-errors#task#11).

Cause 8 of 8: DeletePipe returns DesiredState "DELETED" instead of "STOPPED" and mis-serializes CreationTime/LastModifiedTime.
  AWS: recorded behavior only — DeletePipe (Smithy: DELETE /v1/pipes/{Name}, 200, output DeletePipeResponse with DesiredState: RequestedPipeStateDescribeResponse one of RUNNING|STOPPED|DELETED) answers the delete request with DesiredState "STOPPED" while CurrentState moves to the deleting state; CreationTime/LastModifiedTime are Timestamp members asserted by shape. Hypothesis: AWS reports the requested run-state of the pipe being torn down, not the terminal lifecycle marker.
  Emulator: DesiredState "DELETED", and both timestamps as bare epoch-seconds floats (e.g. 1793344203.158 / 1793344206.211).
  Why: a client that polls deletion by comparing DeletePipe's DesiredState against subsequent describes takes a different branch than on AWS, and the numeric timestamps fail strict deserialization as in cause 1.
  Fix: DeletePipe's handler/response projection (src/ops/delete_pipe.rs) and the shared timestamp serialization.
  Traces: pipes-list-pipes-pagination#teardown#17 (also pipes-list-pipes-pagination#teardown#18/#19).

## Evidence
Replay of simparity-wire-aws-v1, 8 mismatch clusters, all expected_status == actual_status == 200 (body-only divergences), 24 affected steps:
- DescribePipe, rep pipes-error-conflict-and-source-notfound#task#11 (affected 7): "$.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1793344196.198); $.EnrichmentParameters: missing (expected {}, got None); $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1793344196.198); $.SourceParameters: missing (expected {'SqsQueueParameters': {'BatchSize': 10}}, got None); $.StateReason: missing (expected 'USER_INITIATED', got None); $.Tags: missing (expected {}, got None); $.TargetParameters: missing (expected {}, got None)".
- DescribePipe, rep pipes-filter-criteria-nested-json-preserved#task#8 (affected 1): same set with "$.SourceParameters.SqsQueueParameters: missing (expected {'BatchSize': 10}, got None)" and both timestamps as 1793344198.725.
- ListPipes, rep pipes-list-pipes-filtering#task#15 (affected 6): "$.Pipes[Arn=\"arn:aws:pipes:...:pipe/...\"].StateReason: missing (expected 'USER_INITIATED', got None)".
- DescribePipe, rep pipes-pipe-lifecycle-immediate-conflicts#task#10 (affected 2): EnrichmentParameters/StateReason/Tags/TargetParameters missing.
- DescribePipe, rep pipes-pipe-tag-management#task#8 (affected 1): EnrichmentParameters/SourceParameters/StateReason/TargetParameters missing.
- DescribePipe, rep pipes-sqs-source-eventbus-target#task#7 (affected 1): EnrichmentParameters/SourceParameters/StateReason/Tags missing.
- DescribePipe, rep pipes-update-pipe-config#task#10 (affected 3): EnrichmentParameters/SourceParameters/StateReason/Tags/TargetParameters missing.
- DeletePipe, rep pipes-list-pipes-pagination#teardown#17 (affected 3): "$.DesiredState: value_mismatch (expected 'STOPPED', got 'DELETED')" plus CreationTime/LastModifiedTime shaped-assertion failures against 1793344203.158 / 1793344206.211.
The recorded ARNs, pipe names, BatchSize value and epoch numbers above are per-run fillers; the requirement is that each modeled member is present with the value the request/state supplies, that StateReason carries the state-reason token AWS reports, that timestamps use the modeled date-time representation, and that DeletePipe reports DesiredState STOPPED.

## Steps to Reproduce
1. Replay fixture pipes-error-conflict-and-source-notfound, step task#11: create a pipe with an SQS source (BatchSize set) and call DescribePipe. Response is 200 but SourceParameters, TargetParameters, EnrichmentParameters, Tags and StateReason are absent and CreationTime/LastModifiedTime are bare epoch floats.
2. Replay pipes-filter-criteria-nested-json-preserved#task#8 — nested FilterCriteria pipe: SourceParameters.SqsQueueParameters absent.
3. Replay pipes-list-pipes-filtering#task#15 — ListPipes: every Pipes[] entry is missing StateReason.
4. Replay pipes-pipe-lifecycle-immediate-conflicts#task#10, pipes-pipe-tag-management#task#8, pipes-sqs-source-eventbus-target#task#7 and pipes-update-pipe-config#task#10 — DescribePipe after start/stop, after tagging, for an SQS->EventBus pipe, and after UpdatePipe: the same members are absent in each.
5. Replay pipes-list-pipes-pagination#teardown#17 — DeletePipe: DesiredState is "DELETED" (AWS: "STOPPED") and the timestamps are numeric.
