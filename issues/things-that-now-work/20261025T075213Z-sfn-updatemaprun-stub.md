Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.map-run-tolerated-failure-enforced stepfunctions.update-map-run-adjusts-concurrency-limits
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|UpdateMapRun|NotImplemented
Issue-Commit: 62a489da4fc45e0eadb1189eb41434873a830e66

# Implement sfn UpdateMapRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `62a489da4fc45e0eadb1189eb41434873a830e66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMapRun

## Description
`UpdateMapRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/update_map_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateMapRun` call, so each one unblocks only when `UpdateMapRun` works: `sfn.map-run-tolerated-failure-enforced`, `sfn.update-map-run-adjusts-concurrency-limits`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/update_map_run.rs`. Replaying the corpus, the emulator refused the `UpdateMapRun` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateMapRun operation: operation UpdateMapRun is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call UpdateMapRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
