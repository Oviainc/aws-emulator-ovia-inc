Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal events.start-replay-end-time-before-start-rejected events.start-replay-invalid-source-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CancelReplay|NotImplemented
Issue-Key: events.start-replay-end-time-before-start-rejected|run#23|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: events.start-replay-invalid-source-rejected|run#20|AssertionError|assert err.get("Code") == "ValidationException", f"expected ValidationException, got {err}"
Issue-Commit: 1f7e5adccf9e411825240a72188af4d50262df19

# EventBridge replay: CancelReplay is a 501 stub, and StartReplay's two refusal paths answer the wrong code and the wrong message (3 causes)

> _Note: this issue was identified against the code base at commit `1f7e5adccf9e411825240a72188af4d50262df19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplay

## Description
Three independent root causes in the EventBridge replay family. None of the three checks is marked OWNED or ALREADY FILED in the replay report. All three check ids appear in the Issue-Trace-Ids of the open issue 20261025T084539Z-events-eventbridge-archive-and-replay-control-plane-is-unimplemented.md, but that issue describes them all as `StartReplay is a scaffold stub` — StartReplay is now IMPLEMENTED (services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs is a full handler, see the fixed issue 20261025T081516Z-events-sqs-eventbridge-archive-replay-is-a-501-scaffold-stub.md), so these three failures are new signatures that issue does not describe: a different operation is now the stub, and the two refusal paths StartReplay grew are wrong.

Cause 1 of 3: CancelReplay is a 501 scaffold stub, so a started replay has no exit other than running to completion.
AWS: Smithy (com.amazonaws.eventbridge#CancelReplay) models input {ReplayName required, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$} and output {ReplayArn, State (ReplayState: STARTING|RUNNING|COMPLETED|CANCELLING|CANCELLED|FAILED), StateReason}; its modeled errors are exactly ConcurrentModificationException (400), IllegalStatusException (400), InternalException (500) and ResourceNotFoundException (400). Cancelling a STARTING/RUNNING replay answers HTTP 200 with State=CANCELLING; the replay then settles in CANCELLED (not COMPLETED) with a ReplayEndTime, ListReplays/DescribeReplay agree, a second CancelReplay on the now-terminal replay is refused with IllegalStatusException, and the replay name stays taken so StartReplay reusing it is ResourceAlreadyExistsException. NotImplemented/501 is not in the modeled error set for this operation.
Emulator: `An error occurred (NotImplemented) when calling the CancelReplay operation: operation CancelReplay is scaffolded but not implemented in this emulator` (HTTP 501). The handler file still carries the generated `SCAFFOLDED, NOT IMPLEMENTED` marker.
Why: a client that starts a long replay by mistake, or must stop a backfill that is flooding its consumers, has no way to stop it — and the CANCELLED branch of the replay state machine (and the IllegalStatusException that guards re-cancelling a terminal replay) is unreachable. Because 501/NotImplemented is unmodeled, an SDK cannot map it onto any CancelReplay failure it knows how to handle.
Fix: the CancelReplay handler, services/service-com-amazonaws-eventbridge-awsevents/src/ops/cancel_replay.rs (and its identically scaffolded sibling under services/service-com-amazonaws-cloudwatchevents-awsevents/src/ops/), plus the replay state machine it must drive (CANCELLING -> CANCELLED, terminal, with ReplayEndTime) in the crate's replay state.
Traces: events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal.

Cause 2 of 3: StartReplay's inverted-window refusal carries the wrong message text.
AWS: recorded behavior only (the model declares no ValidationException for StartReplay; AWS answers the service-common ValidationException). A StartReplay whose EventEndTime is not later than its EventStartTime is refused with the message `Parameter EventEndTime is not valid. Reason: EventStartTime must be before EventEndTime.` The fixed text is `Parameter EventEndTime is not valid. Reason: EventStartTime must be before EventEndTime.`; it has NO variable parts (the member name EventEndTime is the member that was refused, and the reason clause names the two member names, not their values).
Emulator: the same refusal, but with the reason clause inverted: `Parameter EventEndTime is not valid. Reason: EventEndTime must be later than EventStartTime.` The text is hardcoded at services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs lines 205-210.
Why: clients and operators match on the Reason clause to classify which parameter rule they broke; a reworded reason breaks log/alarm matching and makes the emulator's refusal non-interchangeable with AWS's for any consumer that asserts on the message.
Fix: the StartReplay handler's window validation — the refusal message text only; the status (400) and the code are already right.
Traces: events.start-replay-end-time-before-start-rejected.

Cause 3 of 3: StartReplay naming an archive that does not exist answers ResourceNotFoundException with a bare message, where AWS answers ValidationException with a parameter-scoped message.
AWS: recorded behavior only for the classification (Smithy does model ResourceNotFoundException for StartReplay, but AWS does not use it for an EventSourceArn that resolves to no archive). AWS refuses with HTTP 400, Error.Code = `ValidationException` and the message `Parameter EventSourceArn is not valid. Reason: Archive <archive-name> does not exist.` — fixed text `Parameter EventSourceArn is not valid. Reason: Archive ` + the archive name taken from the EventSourceArn in the request + ` does not exist.`. The rejected call must also create no replay (ListReplays under that name stays empty).
Emulator: HTTP 400 with Error.Code = `ResourceNotFoundException` and Message = `Archive <archive-name> does not exist.` — the code is wrong and the message drops the `Parameter EventSourceArn is not valid. Reason: ` prefix. Produced at services/service-com-amazonaws-eventbridge-awsevents/src/ops/start_replay.rs lines 239-249 (crate::state::no_such_archive).
Why: a client branching on Error.Code takes the retry/not-found branch instead of the input-validation branch for what AWS treats as a malformed parameter, and anything asserting on the message text sees a different message class entirely.
Fix: the StartReplay handler's EventSourceArn resolution — classify an unresolvable archive ARN as the service-common ValidationException with the parameter-scoped message instead of the modeled ResourceNotFoundException.
Traces: events.start-replay-invalid-source-rejected.

## Evidence
Cause 1 — events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CancelReplay operation: operation CancelReplay is scaffolded but not implemented in this emulator
Expected: HTTP 200 {ReplayArn, State='CANCELLING'}, the replay then settling at CANCELLED with ReplayEndTime, ListReplays agreeing, a second CancelReplay -> IllegalStatusException, and StartReplay reusing the name -> ResourceAlreadyExistsException.
Actual: HTTP 501, Error.Code = NotImplemented (unmodeled for this operation). services/service-com-amazonaws-eventbridge-awsevents/src/ops/cancel_replay.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4).

Cause 2 — events.start-replay-end-time-before-start-rejected [mismatch]:
  AssertionError: expected message 'Parameter EventEndTime is not valid. Reason: EventStartTime must be before EventEndTime.', got 'Parameter EventEndTime is not valid. Reason: EventEndTime must be later than EventStartTime.'
Expected: 400 ValidationException, message exactly `Parameter EventEndTime is not valid. Reason: EventStartTime must be before EventEndTime.` (all fixed text).
Actual: 400 with the reason clause reworded to `EventEndTime must be later than EventStartTime.`

Cause 3 — events.start-replay-invalid-source-rejected [mismatch]:
  AssertionError: expected ValidationException, got {'Message': 'Archive sim2real-behav-archive-<id>-does-not-exist does not exist.', 'Code': 'ResourceNotFoundException'}
Expected: Error.Code = 'ValidationException', Message = `Parameter EventSourceArn is not valid. Reason: Archive <archive-name>-does-not-exist does not exist.` (fixed prefix/suffix; the archive name comes from the request's EventSourceArn). The positive control StartReplay against the real archive succeeds in both AWS and the emulator, so the divergence is isolated to the bogus-source branch.
Actual: Error.Code = 'ResourceNotFoundException', Message = `Archive <archive-name> does not exist.`

## Steps to Reproduce
Cause 1:
1. events.create_event_bus(Name=B); events.create_archive(ArchiveName=A, EventSourceArn=<bus arn>); poll describe_archive until State='ENABLED'.
2. events.put_events(...) onto B with past Time values; events.start_replay(ReplayName=R, EventSourceArn=<archive arn>, EventStartTime/EventEndTime bracketing those times, Destination={'Arn': <bus arn>}) -> now accepted.
3. events.cancel_replay(ReplayName=R) -> HTTP 501 NotImplemented. Expected 200 with State='CANCELLING', then describe_replay(R) settling at 'CANCELLED' with ReplayEndTime; cancel_replay(R) again -> IllegalStatusException.

Cause 2:
1. Create a bus and an ENABLED archive on it.
2. events.start_replay(ReplayName=R, EventSourceArn=<archive arn>, EventStartTime=now, EventEndTime=now-1h, Destination={'Arn': <bus arn>}).
3. Read Error.Message -> 'Parameter EventEndTime is not valid. Reason: EventEndTime must be later than EventStartTime.' Expected '... Reason: EventStartTime must be before EventEndTime.'

Cause 3:
1. Create a bus and an ENABLED archive A; confirm start_replay against A's real ARN succeeds (positive control).
2. events.start_replay(ReplayName=R2, EventSourceArn=<A's arn> + '-does-not-exist', EventStartTime=now-1h, EventEndTime=now, Destination={'Arn': <bus arn>}).
3. Read Error.Code -> 'ResourceNotFoundException'. Expected 'ValidationException' with Message 'Parameter EventSourceArn is not valid. Reason: Archive <name>-does-not-exist does not exist.'; then list_replays(NamePrefix=R2) must be empty.

Replay the three check ids in trace_ids to confirm.
