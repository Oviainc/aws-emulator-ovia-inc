Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.hook-result-payload-blob-round-trip
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.hook-result-payload-blob-round-trip|run#4|AssertionError|assert error.get("Code") == "HookResultNotFound", f"expected HookResultNotFound, got {error!r}"
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation GetHookResult: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHookResult

## Description
`GetHookResult` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/get_hook_result.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetHookResult` call, so each one unblocks only when `GetHookResult` works: `cloudformation.hook-result-payload-blob-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/get_hook_result.rs`. Replaying the corpus, the emulator refused the `GetHookResult` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudformation_hook_result_payload_blob_round_trip.py", line 44, in run
    assert error.get("Code") == "HookResultNotFound", f"expected HookResultNotFound, got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected HookResultNotFound, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation GetHookResult is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the cloudformation emulator
Call GetHookResult and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
