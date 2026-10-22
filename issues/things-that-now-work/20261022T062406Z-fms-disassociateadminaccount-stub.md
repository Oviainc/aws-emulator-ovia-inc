Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.disassociate-admin-account-unconfigured-not-found
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.disassociate-admin-account-unconfigured-not-found|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}: {shown}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Implement fms DisassociateAdminAccount: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateAdminAccount

## Description
`DisassociateAdminAccount` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/disassociate_admin_account.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisassociateAdminAccount` call, so each one unblocks only when `DisassociateAdminAccount` works: `fms.disassociate-admin-account-unconfigured-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/disassociate_admin_account.rs`. Replaying the corpus, the emulator refused the `DisassociateAdminAccount` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_fms_disassociate_admin_account_unconfigured_not_found.py", line 34, in run
    assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}: {shown}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException, got NotImplemented: operation DisassociateAdminAccount is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the fms emulator
Call DisassociateAdminAccount and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
