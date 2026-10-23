Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.multiplex-start-without-programs-rejected
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.multiplex-start-without-programs-rejected|run#15|AssertionError|assert code == "ConflictException", f"expected ConflictException deleting a running multiplex, got {code}"
Issue-Commit: 638adeb47fb8bd46dfae7bf3f85b8f37e7c32e95

# medialive DeleteMultiplex is an unimplemented scaffold stub (501 NotImplemented instead of the modeled 202/ConflictException delete semantics)

> _Note: this issue was identified against the code base at commit `638adeb47fb8bd46dfae7bf3f85b8f37e7c32e95`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMultiplex

## Description
One root cause, affecting the medialive multiplex lifecycle.

Cause 1 of 1: `DeleteMultiplex` is still a scaffold stub, so every multiplex delete — accepted or rejected — answers HTTP 501 `NotImplemented` and the state-dependent delete semantics around it are not exercised at all. Failing check: medialive.multiplex-start-without-programs-rejected.

AWS: Smithy model (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json) binds `DeleteMultiplex` to `DELETE /prod/multiplexes/{MultiplexId}` with success code 202, input `MultiplexId` (__string, @httpLabel, required), and output members Arn/AvailabilityZones/Destinations/Id/MultiplexSettings/Name/PipelinesRunningCount/ProgramCount/State (MultiplexState: CREATE_FAILED|CREATING|DELETED|DELETING|IDLE|RECOVERING|RUNNING|STARTING|STOPPING)/Tags. The operation's documented contract is "Delete a multiplex. The multiplex must be idle.", and it models `ConflictException` (HTTP 409) and `NotFoundException` (HTTP 404) among its errors. Recorded behavior for the state machine around it: deleting a multiplex in state RUNNING is refused with `ConflictException` (409) and has NO effect (a following DescribeMultiplex still reports State RUNNING); after StopMultiplex settles the multiplex to IDLE the identical DeleteMultiplex call is accepted (202); the delete is asynchronous, so the id disappears from ListMultiplexes promptly while DescribeMultiplex keeps reporting it with State DELETING, and a mutation against that DELETING id (e.g. StartMultiplex) is refused with `ConflictException` (409) rather than `NotFoundException`. The recorded messages ("Cannot delete a multiplex that is in RUNNING state", "Multiplex <id> cannot be started while in DELETING state") are examples of AWS wording, not the requirement — the requirements are the statuses and error codes.

Emulator: `DeleteMultiplex::handle` returns `crate::scaffolded::Scaffolded::not_implemented("DeleteMultiplex")`, so the call fails with HTTP 501 and error code `NotImplemented` regardless of multiplex state. Replay: the check creates a multiplex, waits for IDLE, starts it (StartMultiplex and DescribeMultiplex work: State RUNNING, ProgramCount 0, PipelinesRunningCount >= 1), then calls DeleteMultiplex while RUNNING and gets `NotImplemented` — `AssertionError: expected ConflictException deleting a running multiplex, got NotImplemented`. Because the call never completes, the downstream facts (no-effect on reject, delete-after-stop succeeding, removal from ListMultiplexes, and ConflictException on a mutation of a DELETING id) are unreachable.

Why: a client cannot tear down a multiplex at all against the emulator, and cannot observe the state guard it relies on. Teardown/cleanup code sees an unmodeled 501 `NotImplemented` (not in the operation's modeled error set) where AWS returns either a 202 success body or a modeled 409 `ConflictException`, so SDK retry/error-branching logic takes a path it never takes against AWS, and test suites that create multiplexes leak them. Code that distinguishes "still settling" (409 on a DELETING id) from "gone" (404) cannot be written or validated.

Fix: implement the operation's handler, `services/service-com-amazonaws-medialive-medialive/src/ops/delete_multiplex.rs` (and remove its SCAFFOLDED marker line), returning the modeled output shape with 202 on an IDLE multiplex and `DeleteMultiplexErrorModeled::ConflictException` for a non-idle one; the multiplex state transition RUNNING -> (StopMultiplex) -> IDLE -> (DeleteMultiplex) -> DELETING -> DELETED, the ListMultiplexes-vs-DescribeMultiplex visibility split, and the ConflictException guard for mutations on a DELETING id belong in this service's multiplex state (crate `state`/multiplex store) shared with StartMultiplex/StopMultiplex/DescribeMultiplex/ListMultiplexes.

Traces: medialive.multiplex-start-without-programs-rejected (mismatch; assertion at check line 52, `expected ConflictException deleting a running multiplex, got NotImplemented`).

Related but distinct open issues, which do NOT cover this operation: 20261023T003754Z-medialive-listmultiplexprograms-stub.md, 20261023T003754Z-medialive-updatemultiplexprogram-stub.md, 20261023T003849Z-medialive-createcloudwatchalarmtemplate-stub--r2.md (other stubbed operations), and 20261023T003754Z-medialive-rejection-messages-diverge-from-aws-wording-across.md (message wording where the code is already correct — here the code itself is wrong).

## Evidence
Expected (AWS / Smithy model): DeleteMultiplex on a RUNNING multiplex -> HTTP 409, error code `ConflictException`, no effect (DescribeMultiplex still State=RUNNING); after StopMultiplex -> IDLE, the same DeleteMultiplex -> HTTP 202 with the multiplex payload; id then absent from ListMultiplexes while DescribeMultiplex reports State=DELETING; StartMultiplex on that DELETING id -> HTTP 409 `ConflictException` (not NotFoundException).

Actual (emulator): HTTP 501, error code `NotImplemented`, message "operation DeleteMultiplex is scaffolded but not implemented in this emulator", for every DeleteMultiplex call irrespective of state. Handler body: `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteMultiplex"))` in services/service-com-amazonaws-medialive-medialive/src/ops/delete_multiplex.rs (line 177); the SCAFFOLDED marker is still present at line 4.

Replay failure: botocore ClientError Code=`NotImplemented` surfaced as `AssertionError: expected ConflictException deleting a running multiplex, got NotImplemented`.

## Steps to Reproduce
1. Boot the emulator and point a botocore medialive client at it.
2. CreateMultiplex(Name=<unique>, AvailabilityZones=<two AZs from ec2 DescribeAvailabilityZones>, RequestId=<unique>, MultiplexSettings={TransportStreamBitrate: 10000000, TransportStreamId: 1}); poll DescribeMultiplex until State == IDLE.
3. StartMultiplex(MultiplexId=<id>); poll DescribeMultiplex until State == RUNNING (observed: works, ProgramCount 0, PipelinesRunningCount >= 1).
4. DeleteMultiplex(MultiplexId=<id>) while State == RUNNING.
   Expected: HTTP 409 with Error.Code == "ConflictException", multiplex still RUNNING.
   Actual: HTTP 501 with Error.Code == "NotImplemented".
5. (Blocked by step 4) StopMultiplex -> wait IDLE -> DeleteMultiplex: expect HTTP 202; then ListMultiplexes should no longer list the id while DescribeMultiplex reports State == DELETING, and StartMultiplex on that id should return HTTP 409 ConflictException.

Replay: run_behavioral_suite and re-run check medialive.multiplex-start-without-programs-rejected.
