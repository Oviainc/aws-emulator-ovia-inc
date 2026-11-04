Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-deleteApplicationStatusCheck ec2.op-contract-describe-application-status-checks-duplicate
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-deleteApplicationStatusCheck|run#6|AssertionError|assert err.get("Code") == "DryRunOperation", f"expected DryRunOperation, got {err!r}"
Issue-Key: ec2|DeleteApplicationStatusCheck|NotImplemented
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# Implement ec2 DeleteApplicationStatusCheck: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplicationStatusCheck

## Description
`DeleteApplicationStatusCheck` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_application_status_check.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteApplicationStatusCheck` call, so each one unblocks only when `DeleteApplicationStatusCheck` works: `ec2.op-contract-deleteApplicationStatusCheck`, `ec2.op-contract-describe-application-status-checks-duplicate`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_application_status_check.rs`. Replaying the corpus, the emulator refused the `DeleteApplicationStatusCheck` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_ec2_op_contract_deleteApplicationStatusCheck.py", line 35, in run
    assert err.get("Code") == "DryRunOperation", f"expected DryRunOperation, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected DryRunOperation, got {'Code': 'NotImplemented', 'Message': 'operation DeleteApplicationStatusCheck is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteApplicationStatusCheck and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
