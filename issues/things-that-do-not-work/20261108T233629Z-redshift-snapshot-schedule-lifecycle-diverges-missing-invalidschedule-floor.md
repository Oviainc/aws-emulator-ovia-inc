Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-snapshot-schedule-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-snapshot-schedule-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|CreateSnapshotSchedule|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateSnapshotSchedule|4xx->2xx|status
Issue-Key: redshift|CreateSnapshotSchedule|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeSnapshotSchedules|2xx->2xx|$.DescribeSnapshotSchedulesResponse.DescribeSnapshotSchedulesResult.SnapshotSchedules.SnapshotSchedule.ScheduleDefinitions.ScheduleDefinition
Issue-Key: redshift|DescribeSnapshotSchedules|2xx->2xx|$.DescribeSnapshotSchedulesResponse.DescribeSnapshotSchedulesResult.SnapshotSchedules
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# Redshift snapshot-schedule lifecycle diverges: missing InvalidSchedule floor, emulator-authored already-exists wording, and modify/delete never reach state

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshotSchedule

## Description
This bundle carries FOUR root causes in the Redshift snapshot-schedule surface: two refusals the emulator authors itself (one missing entirely, one worded wrongly) and two state mutations that never reach the store, so the read-back of a schedule is stale. All four must be closed before this issue is submitted.

Cause 1 of 4: CreateSnapshotSchedule accepts a sub-hourly rate() definition instead of refusing it with InvalidSchedule (fixture redshift-snapshot-schedule-lifecycle, step task#6).
AWS: a Redshift snapshot schedule fires at most once an hour, so a `rate(<value> <unit>)` definition stating an interval shorter than 60 minutes is refused and nothing is created. The refusal is the operation's modeled fault - Smithy: CreateSnapshotSchedule errors include InvalidScheduleFault (errorKind client, httpError 400, awsQueryError code `InvalidSchedule`), documented "The schedule you submitted isn't valid." The model states no numeric bound on CreateSnapshotScheduleMessage$ScheduleDefinitions (ScheduleDefinitionList of String, no length/pattern), so the 60-minute floor is recorded behavior only: the recorded response is HTTP 400 with Error.Type `Sender`, Error.Code `InvalidSchedule` and Error.Message "Schedule interval rate(3 minutes) is less than minimum interval of 60 minutes". In that message the FIXED text is "Schedule interval " ... " is less than minimum interval of 60 minutes"; the only variable part is the offending definition, echoed verbatim from the request's ScheduleDefinitions entry (`rate(3 minutes)` is this recording's request value, not part of the template). The `60 minutes` bound is the rule itself and is fixed text.
Emulator: the call is SERVED - HTTP 200 with the schedule echoed back and no ErrorResponse element at all (replay: "status: expected 400, got 200"; "$.ErrorResponse: missing"). The sub-hourly schedule is then stored and visible to DescribeSnapshotSchedules.
Why: a client that relies on AWS's rejection to validate operator- or template-supplied backup cadences gets a false positive. Infrastructure code and tests that assert the refusal pass against the emulator and fail against AWS, and emulator-side snapshot workloads run at a cadence AWS cannot produce.
Fix: the schedule-definition validation shared by the create path (the rate branch of the snapshot-schedule definition parser, lifted by the CreateSnapshotSchedule handler to InvalidScheduleFault with the minimum-interval message, writing nothing). The same floor must hold on every path that accepts a definition, ModifySnapshotSchedule included, and on the DryRun form. NOTE: open behavioral issue 20261108T170214Z-redshift-createsnapshotschedule-accepts-a-sub-hourly-rate-definition-instead.md describes this same root cause but carries NO wire replay key, so this wire step is not covered by it and must be re-replayed here.

Cause 2 of 4: the duplicate-identifier refusal for CreateSnapshotSchedule carries an emulator-authored message (fixture redshift-snapshot-schedule-lifecycle, step task#5).
AWS: creating a schedule whose ScheduleIdentifier already exists is refused with the modeled SnapshotScheduleAlreadyExistsFault - Smithy: CreateSnapshotSchedule errors include SnapshotScheduleAlreadyExistsFault (errorKind client, httpError 400, awsQueryError code `SnapshotScheduleAlreadyExists`), documented "The specified snapshot schedule already exists." The wording is recorded behavior only: the recorded response is HTTP 400 with Error.Message "Snapshot schedule <ScheduleIdentifier> already exists." - FIXED text "Snapshot schedule " and " already exists." (with the trailing period), the single variable part being the ScheduleIdentifier echoed from the request (the recording's `sim2real-probe-redshift-ss-3ea4183d` is a request value, not part of the template).
Emulator: the status and error code match, but the message is the emulator's own sentence: "Cannot create the snapshot schedule because a schedule with the identifier '<ScheduleIdentifier>' already exists." - different fixed text, different clause order, and the identifier quoted where AWS leaves it bare.
Why: clients and operators that match or display refusal text (log scrapers, idempotency wrappers that recognise the already-exists sentence, snapshot-tested CLI output) see a message no AWS region ever emits, so error handling keyed on AWS's wording does not fire.
Fix: the CreateSnapshotSchedule handler's duplicate-identifier refusal message, emitted with the SnapshotScheduleAlreadyExistsFault mapping.

Cause 3 of 4: ModifySnapshotSchedule's new definition never reaches state, so DescribeSnapshotSchedules keeps reporting the create-time definition (fixture redshift-snapshot-schedule-lifecycle, step task#4).
AWS: ModifySnapshotSchedule replaces the schedule's ScheduleDefinitions - Smithy: ModifySnapshotSchedule input ScheduleIdentifier (required) + ScheduleDefinitions (required, ScheduleDefinitionList), output shape SnapshotSchedule, errors InvalidScheduleFault / SnapshotScheduleNotFoundFault / SnapshotScheduleUpdateInProgressFault. After a successful modify, DescribeSnapshotSchedules must report the definition last submitted; the recorded read shows SnapshotSchedules.SnapshotSchedule.ScheduleDefinitions.ScheduleDefinition equal to the value the preceding modify submitted (`rate(24 hours)` in this recording - a request value, not part of the requirement).
Emulator: the read-back still carries the definition the schedule was CREATED with (replay: "ScheduleDefinitions.ScheduleDefinition: value_mismatch (expected 'rate(24 hours)', got 'rate(12 hours)')"). The modify is not applied to the stored schedule. Hypothesis: the modify path is the unimplemented scaffold already filed as 20261108T170214Z-redshift-modifysnapshotschedule-stub.md, so nothing mutates state; that stub issue covers the modify call's own response, NOT this read-back step, which has no replay key on it.
Why: any client that edits a backup cadence and then re-reads it (console-style refresh, Terraform/CloudFormation drift detection, a test that asserts the new cadence) observes the old cadence indefinitely, so an applied change looks silently reverted.
Fix: the ModifySnapshotSchedule handler and the snapshot-schedule state module - the update must replace the stored ScheduleDefinitions (and recompute NextInvocations) under the same definition validation as create, so the describe path serves the new definition.

Cause 4 of 4: a deleted snapshot schedule is still listed by DescribeSnapshotSchedules (fixture redshift-snapshot-schedule-lifecycle, step teardown#10).
AWS: DeleteSnapshotSchedule removes the schedule - Smithy: DeleteSnapshotSchedule input ScheduleIdentifier (required), output Unit, errors SnapshotScheduleNotFoundFault / InvalidClusterSnapshotScheduleStateFault. After the delete, DescribeSnapshotSchedules answers HTTP 200 with an EMPTY SnapshotSchedules element (the recorded teardown read shows SnapshotSchedules serialized as the empty list).
Emulator: the teardown read answers 200 but still returns the schedule record (replay: "SnapshotSchedules: type_mismatch (expected '', got {'SnapshotSchedule': {...ScheduleIdentifier: <the deleted id>, ScheduleDefinitions: rate(12 hours), AssociatedClusterCount: '0', ...}})"). The schedule survives its own deletion, and with the stale definition of Cause 3.
Why: a client cannot observe deletion: cleanup loops that poll until the list is empty never terminate, a re-create of the same identifier hits the already-exists refusal of Cause 2, and quota accounting for schedules never recovers.
Fix: the DeleteSnapshotSchedule handler and the snapshot-schedule state module - the delete must remove the record (and its cluster associations) from the store so the describe path serves an empty SnapshotSchedules list.

Traces: redshift-snapshot-schedule-lifecycle#task#6 (Cause 1, representative), redshift-snapshot-schedule-lifecycle#task#5 (Cause 2), redshift-snapshot-schedule-lifecycle#task#4 (Cause 3), redshift-snapshot-schedule-lifecycle#teardown#10 (Cause 4).

## Evidence
Cause 1 - CreateSnapshotSchedule (fixture redshift-snapshot-schedule-lifecycle, step task#6), ScheduleDefinitions containing a sub-hourly rate (recording: ["rate(3 minutes)"]):
  Expected (recorded AWS): HTTP 400; ErrorResponse.Error = {Type: Sender, Code: InvalidSchedule, Message: "Schedule interval rate(3 minutes) is less than minimum interval of 60 minutes"} - fixed text "Schedule interval " + the request's definition + " is less than minimum interval of 60 minutes".
  Actual (emulator): HTTP 200, no ErrorResponse element ($.ErrorResponse: missing), schedule created and stored.

Cause 2 - CreateSnapshotSchedule with an identifier that already exists (step task#5):
  Expected (recorded AWS): HTTP 400, Error.Code SnapshotScheduleAlreadyExists, Error.Message "Snapshot schedule <ScheduleIdentifier> already exists." (recording: "Snapshot schedule sim2real-probe-redshift-ss-3ea4183d already exists.").
  Actual (emulator): HTTP 400 with Error.Message "Cannot create the snapshot schedule because a schedule with the identifier '<ScheduleIdentifier>' already exists." Only $.ErrorResponse.Error.Message diverges.

Cause 3 - DescribeSnapshotSchedules after ModifySnapshotSchedule (step task#4):
  Expected (recorded AWS): $.DescribeSnapshotSchedulesResponse.DescribeSnapshotSchedulesResult.SnapshotSchedules.SnapshotSchedule.ScheduleDefinitions.ScheduleDefinition = the definition the preceding modify submitted (recording: 'rate(24 hours)').
  Actual (emulator): 'rate(12 hours)' - the create-time definition; value_mismatch on that key.

Cause 4 - DescribeSnapshotSchedules after DeleteSnapshotSchedule (step teardown#10):
  Expected (recorded AWS): HTTP 200 with SnapshotSchedules empty ('').
  Actual (emulator): HTTP 200 with SnapshotSchedules = {SnapshotSchedule: {ScheduleIdentifier: <the deleted id>, ScheduleDefinitions: {ScheduleDefinition: 'rate(12 hours)'}, ScheduleDescription: ..., Tags: '', NextInvocations: {...}, AssociatedClusterCount: '0', AssociatedClusters: ''}} - type_mismatch on $...SnapshotSchedules.

Model basis: Smithy CreateSnapshotSchedule errors {InvalidScheduleFault -> 400/InvalidSchedule, SnapshotScheduleAlreadyExistsFault -> 400/SnapshotScheduleAlreadyExists, ...}; ModifySnapshotSchedule input ScheduleIdentifier+ScheduleDefinitions both required, output SnapshotSchedule; DeleteSnapshotSchedule input ScheduleIdentifier required, output Unit; DescribeSnapshotSchedules output SnapshotSchedules (SnapshotScheduleList), paginated by Marker/MaxRecords.

## Steps to Reproduce
1. Boot the emulator and point a redshift client at its data listener.
2. Cause 2 and 3 setup: CreateSnapshotSchedule with a unique ScheduleIdentifier, a ScheduleDescription and ScheduleDefinitions=["rate(12 hours)"] - succeeds on both.
3. Cause 3: ModifySnapshotSchedule on that identifier with ScheduleDefinitions=["rate(24 hours)"], then DescribeSnapshotSchedules(ScheduleIdentifier=<that id>). Observe ScheduleDefinitions still reporting the create-time 'rate(12 hours)'; AWS reports the modified definition.
4. Cause 2: CreateSnapshotSchedule again with the SAME ScheduleIdentifier. Observe HTTP 400 with Message "Cannot create the snapshot schedule because a schedule with the identifier '<id>' already exists."; AWS answers "Snapshot schedule <id> already exists."
5. Cause 1: CreateSnapshotSchedule with a fresh identifier and ScheduleDefinitions=["rate(3 minutes)"]. Observe HTTP 200 with the schedule echoed back; AWS answers HTTP 400, Error.Code InvalidSchedule, Error.Message "Schedule interval rate(3 minutes) is less than minimum interval of 60 minutes". Any sub-hourly rate reproduces it; "rate(1 hour)" is accepted by both.
6. Cause 4: DeleteSnapshotSchedule(ScheduleIdentifier=<the id from step 2>), then DescribeSnapshotSchedules(ScheduleIdentifier=<that id>). Observe the schedule still listed; AWS returns an empty SnapshotSchedules list.
7. Replay the bundled fixture steps: redshift-snapshot-schedule-lifecycle#task#6, #task#5, #task#4, #teardown#10.
