Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Multiplex program responses never report packetIdentifiersMap, so the transport-stream PIDs of a created program are unobservable

_Filed: 2026-10-24T01:07:38Z_
_Operation: DescribeMultiplexProgram | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A multiplex program's `PacketIdentifiersMap` (`MultiplexProgramPacketIdentifiersMap`: videoPid, audioPids, pcrPid, pmtPid, privateMetadataPid, scte27Pids, scte35Pid, timedMetadataPid, dvbSubPids, dvbTeletextPid, etbPid, klvDataPids) is assigned by the service when the program is created and is reported by every read of that program; it is how a caller configures a downstream receiver for the multiplex output.

Expected: CreateMultiplexProgram / DescribeMultiplexProgram / UpdateMultiplexProgram report a populated `packetIdentifiersMap` for a stored program (the PIDs the service allocated, derivable from the program's `ProgramNumber` and its settings).

Actual: the single shared projection `ComAmazonawsMedialiveMedialiveState::multiplex_program_description` (src/state/multiplexes.rs:967-987) hard-sets `packet_identifiers_map: None` (and `pipeline_details: None`) with the comment that the emulator allocates no PIDs. So every multiplex-program response omits the member entirely and a client that reads PIDs from the create or describe response gets nothing, for the whole lifetime of the program. The program's own `MultiplexProgramSettings` are stored and reported, so the omission is only in this derived member — it could be derived deterministically from the stored program instead of being dropped.

## Evidence

Spec: `MultiplexProgram$PacketIdentifiersMap` is declared on the output of CreateMultiplexProgram, DescribeMultiplexProgram and UpdateMultiplexProgram (the structurally identical program-description outputs the crate projects through `multiplex_program_description_output!`), and `MultiplexProgramPacketIdentifiersMap` is a concrete structure of PID members — a value the service, not the caller, supplies. Well-established MediaLive behavior: PIDs are allocated at program creation and returned with the program. Handler/state: src/ops/describe_multiplex_program.rs:157-194 answers from `multiplex_program_description`, which sets the member to None at src/state/multiplexes.rs:976-984; the same function backs CreateMultiplexProgram (ops/create_multiplex_program.rs:207) and UpdateMultiplexProgram (ops/update_multiplex_program.rs:182). No test asserts the member, and no existing issue covers it (the open multiplex issue is the DeleteMultiplexProgram stub).

## Steps to Reproduce

1. POST /prod/multiplexes with two availability zones and valid multiplexSettings; poll GET /prod/multiplexes/<id> until state IDLE.
2. POST /prod/multiplexes/<id>/programs with programName=prog1 and multiplexProgramSettings.programNumber=1 — expected 201 whose multiplexProgram carries packetIdentifiersMap with the allocated PIDs; the emulator's body has no packetIdentifiersMap key.
3. GET /prod/multiplexes/<id>/programs/prog1 — expected 200 with the same packetIdentifiersMap; the emulator again omits it.
4. GET /prod/multiplexes/<id>/programs (ListMultiplexPrograms) then compare: nothing in the API surface ever exposes the program's PIDs.
