Issue-Source: trace-sweep-divergence
Issue-Trace: events-archive-create-configure-verify-delete
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-archive-create-configure-verify-delete events-replay-archived-events-start-monitor-cancel
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateArchive|2xx->2xx|$.State
Issue-Key: events|DescribeArchive|2xx->2xx|$.State
Issue-Key: events|UpdateArchive|2xx->2xx|$.State
Issue-Key: events|ListArchives|2xx->2xx|$.Archives[*]
Issue-Commit: 10534c2dd099d7fcc20a17236779012152da4f21

# events archive lifecycle never settles: CreateArchive, DescribeArchive, UpdateArchive and ListArchives report CREATING/UPDATING instead of ENABLED

> _Note: this issue was identified against the code base at commit `10534c2dd099d7fcc20a17236779012152da4f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateArchive

## Description
An EventBridge archive created through the emulator never leaves its transient state: every reader of the archive (the create response itself, DescribeArchive, UpdateArchive and ListArchives) reports CREATING or UPDATING, and no subsequent call ever reports the settled state. Four causes, all on the archive state machine and the archive serializers that read it.

Cause 1 of 4 — CreateArchive returns State=CREATING.
AWS: Smithy: CreateArchive output member State targets ArchiveState, enum = ENABLED | DISABLED | CREATING | UPDATING | CREATE_FAILED | UPDATE_FAILED. Recorded behavior: CreateArchive answers 200 with State 'ENABLED' for an archive over an existing custom bus with RetentionDays set — the archive is usable as soon as the call returns, so the settled value is what the create response carries. Hypothesis: AWS does expose CREATING for large/slow archives, but not for this shape of request, and in any case it must converge.
Emulator: answers 200 with $.State = 'CREATING'.
Why: a client that polls until State is one of the terminal ArchiveState values (ENABLED/DISABLED/CREATE_FAILED) — which is the documented way to wait for an archive — never terminates, because the emulator has no transition out of CREATING. CloudFormation-style waiters and the AWS CLI's own wait loops hang.
Fix: the CreateArchive handler / archive state in the events crate — the stored archive must be created in its settled state (or be transitioned to it before the first read), not left in the transient one.
Traces: events-archive-create-configure-verify-delete#task#2.

Cause 2 of 4 — DescribeArchive keeps reporting CREATING.
AWS: Smithy: DescribeArchive output State targets ArchiveState. Recorded behavior: DescribeArchive on the just-created archive answers 200 with State 'ENABLED'.
Emulator: answers 200 with $.State = 'CREATING'.
Why: confirms the state never converges — the divergence is not just a first-response artifact, so no amount of polling helps.
Fix: archive state in the events crate (the same transition the DescribeArchive handler reads).
Traces: events-archive-create-configure-verify-delete#task#3 (and task#5, the same cluster).

Cause 3 of 4 — UpdateArchive returns State=UPDATING and the archive stays there.
AWS: Smithy: UpdateArchive output State targets ArchiveState. Recorded behavior: an UpdateArchive that changes RetentionDays/EventPattern answers 200 with State 'ENABLED'.
Emulator: answers 200 with $.State = 'UPDATING'.
Why: same non-convergence on the update path; a client cannot tell a finished update from one still in flight.
Fix: the UpdateArchive handler and the archive state transition it writes.
Traces: events-archive-create-configure-verify-delete#task#4.

Cause 4 of 4 — ListArchives reports the archive in a transient state.
AWS: Smithy: ListArchives returns Archives, each entry carrying ArchiveName, EventSourceArn, State (ArchiveState), RetentionDays, SizeBytes, EventCount, CreationTime. Recorded behavior: the listed entry for an archive that has been created and updated carries State 'ENABLED'; SizeBytes 0 and EventCount 0 for an archive with no archived events.
Emulator: the listed entry is identical except State = 'UPDATING', so the expected list element is reported missing and an entry differing only in State is reported in its place.
Why: list-based reconciliation (inventory, CLI 'list-archives' filters on state) classifies a settled archive as still changing.
Fix: the ListArchives serializer reads the same archive state as Describe; fixing the state machine fixes this reader too — no separate per-reader default.
Traces: events-archive-create-configure-verify-delete#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (48 fixtures, 275 steps, 37 mismatched):
- events|CreateArchive|status=200->200: $.State value_mismatch (expected 'ENABLED', got 'CREATING'); cluster also covers events-replay-archived-events-start-monitor-cancel#setup#2.
- events|DescribeArchive|status=200->200: $.State value_mismatch (expected 'ENABLED', got 'CREATING'); cluster also covers events-archive-create-configure-verify-delete#task#5.
- events|UpdateArchive|status=200->200: $.State value_mismatch (expected 'ENABLED', got 'UPDATING').
- events|ListArchives|status=200->200: $.Archives[0] expected {...'State': 'ENABLED'...}, got {...'State': 'UPDATING'...} (all other members equal; CreationTime shaped-not-asserted).
Status codes all match (200); the divergence is purely the State value in four different readers.

## Steps to Reproduce
1. Replay fixture events-archive-create-configure-verify-delete against the emulator (replay_wire_corpus).
2. task#2 CreateArchive over an existing custom event bus -> inspect $.State.
3. task#3 / task#5 DescribeArchive on that archive -> inspect $.State.
4. task#4 UpdateArchive (change RetentionDays/EventPattern) -> inspect $.State.
5. task#6 ListArchives -> inspect $.Archives[0].State.
Each should report the settled state ENABLED; all four report CREATING or UPDATING.
