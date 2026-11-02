Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items stepfunctions.describe-map-run-default-fields-and-execution-counts stepfunctions.error-invalid-token-list-map-runs stepfunctions.list-map-runs-execution-not-exist stepfunctions.map-run-tolerated-failure-enforced stepfunctions.update-map-run-adjusts-concurrency-limits stepfunctions.update-map-run-validation-exception
Issue-Key: sfn|ListMapRuns|NotImplemented
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# Implement sfn ListMapRuns: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMapRuns

## Description
`ListMapRuns` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/list_map_runs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `ListMapRuns` call, so each one unblocks only when `ListMapRuns` works: `s3__sfn.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items`, `sfn.describe-map-run-default-fields-and-execution-counts`, `sfn.error-invalid-token-list-map-runs`, `sfn.list-map-runs-execution-not-exist`, `sfn.map-run-tolerated-failure-enforced`, `sfn.update-map-run-adjusts-concurrency-limits`, `sfn.update-map-run-validation-exception`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/list_map_runs.rs`. Replaying the corpus, the emulator refused the `ListMapRuns` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListMapRuns operation: operation ListMapRuns is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call ListMapRuns and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
