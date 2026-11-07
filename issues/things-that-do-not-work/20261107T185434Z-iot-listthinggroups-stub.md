Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.thing-group-hierarchy
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.thing-group-hierarchy|_eventually#11|AssertionError|raise AssertionError(f"condition never became true; last error: {last_exc!r}")
Issue-Commit: c67f3bfc3afe65d5816950d789f341a51af57d76

# Implement iot ListThingGroups: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c67f3bfc3afe65d5816950d789f341a51af57d76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListThingGroups

## Description
`ListThingGroups` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_thing_groups.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListThingGroups` call, so each one unblocks only when `ListThingGroups` works: `iot.thing-group-hierarchy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_thing_groups.rs`. Replaying the corpus, the emulator refused the `ListThingGroups` call: File "check_iot_thing_group_hierarchy.py", line 67, in run
    children = _eventually(
               ^^^^^^^^^^^^
  File "check_iot_thing_group_hierarchy.py", line 46, in _eventually
    raise AssertionError(f"condition never became true; last error: {last_exc!r}")
AssertionError: condition never became true; last error: ClientError('An error occurred (NotImplemented) when calling the ListThingGroups operation: operation ListThingGroups is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the iot emulator
Call ListThingGroups and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
