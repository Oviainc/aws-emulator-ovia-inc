Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.activate-certificate-authority-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.activate-certificate-authority-wrong-state-rejected|run#8|AssertionError|assert code == (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement eks ActivateCertificateAuthority: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateCertificateAuthority

## Description
`ActivateCertificateAuthority` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ActivateCertificateAuthority` call, so each one unblocks only when `ActivateCertificateAuthority` works: `eks.activate-certificate-authority-wrong-state-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs`. Replaying the corpus, the emulator refused the `ActivateCertificateAuthority` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_eks_activate_certificate_authority_wrong_state_rejected.py", line 45, in run
    assert code == (
           ^^^^^^^^^
    ...<3 lines>...
    ), f"expected ResourceInUseException, got {code}"
    ^
AssertionError: expected ResourceInUseException, got ('NotImplemented', 'operation ActivateCertificateAuthority is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the eks emulator
Call ActivateCertificateAuthority and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
