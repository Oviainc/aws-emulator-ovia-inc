Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.create-computer-authentication-failed-rejected
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: directory-service|CreateComputer|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement ds CreateComputer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputer

## Description
`CreateComputer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateComputer` call, so each one unblocks only when `CreateComputer` works: `ds.create-computer-authentication-failed-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs`. Replaying the corpus, the emulator refused the `CreateComputer` call: File "check_ds_create_computer_authentication_failed_rejected.py", line 50, in run
    code, message, request_id = _error(
                                ~~~~~~^
        lambda: ds.create_computer(
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^
            DirectoryId=directory_id, ComputerName=computer_name, Password="Sim2Real-Computer-Pw1!"
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        )
        ^
    )
    ^
  File "check_ds_create_computer_authentication_failed_rejected.py", line 108, in _error
    return err["Code"], err.get("Message", ""), exc.response["ResponseMetadata"]["RequestId"]
                                                ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^
KeyError: 'RequestId'

## Steps to Reproduce
Boot the ds emulator
Call CreateComputer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
