Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.delete-permission-not-found
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca.delete-permission-not-found|run#5|AssertionError|assert never_granted == ("ResourceNotFoundException", _NO_PERMISSION_MESSAGE), never_granted
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Implement acm-pca DeletePermission: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePermission

## Description
`DeletePermission` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/delete_permission.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeletePermission` call, so each one unblocks only when `DeletePermission` works: `acm-pca.delete-permission-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/delete_permission.rs`. Replaying the corpus, the emulator refused the `DeletePermission` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_acm_pca_delete_permission_not_found.py", line 77, in run
    assert never_granted == ("ResourceNotFoundException", _NO_PERMISSION_MESSAGE), never_granted
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation DeletePermission is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the acm-pca emulator
Call DeletePermission and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
