Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.disassociate-unassociated-external-connection-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.disassociate-unassociated-external-connection-rejected|run#12|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codeartifact DisassociateExternalConnection: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateExternalConnection

## Description
`DisassociateExternalConnection` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/disassociate_external_connection.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisassociateExternalConnection` call, so each one unblocks only when `DisassociateExternalConnection` works: `codeartifact.disassociate-unassociated-external-connection-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/disassociate_external_connection.rs`. Replaying the corpus, the emulator refused the `DisassociateExternalConnection` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codeartifact_disassociate_unassociated_external_connection_rejected.py", line 53, in run
    assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected ('ResourceNotFoundException', "The external connection 'public:npmjs' is not associated with repository 'sim2real-ca-repo-dff9da9211ef'"), got ('NotImplemented', 'operation DisassociateExternalConnection is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codeartifact emulator
Call DisassociateExternalConnection and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
