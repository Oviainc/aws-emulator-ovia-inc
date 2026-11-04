Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.invoke-act-step-call-id-mismatch-rejected
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act.invoke-act-step-call-id-mismatch-rejected|run#12|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException for an unmatched callId, got {err!r}"
Issue-Commit: e3d5599d128a91029caef4169f509524de4e0ad7

# Implement nova-act InvokeActStep: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e3d5599d128a91029caef4169f509524de4e0ad7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeActStep

## Description
`InvokeActStep` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/invoke_act_step.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `InvokeActStep` call, so each one unblocks only when `InvokeActStep` works: `nova-act.invoke-act-step-call-id-mismatch-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/invoke_act_step.rs`. Replaying the corpus, the emulator refused the `InvokeActStep` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_nova_act_invoke_act_step_call_id_mismatch_rejected.py", line 56, in run
    assert err["Code"] == "ValidationException", f"expected ValidationException for an unmatched callId, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for an unmatched callId, got {'Message': 'operation InvokeActStep is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the nova-act emulator
Call InvokeActStep and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
