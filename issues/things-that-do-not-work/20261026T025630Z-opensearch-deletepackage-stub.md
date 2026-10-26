Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.delete-package-while-associated-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.delete-package-while-associated-rejected|run#16|AssertionError|assert code == "ValidationException", (
Issue-Commit: d5b444aa1a2c0b78901c8918371b89e5e61348ae

# Implement opensearch DeletePackage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5b444aa1a2c0b78901c8918371b89e5e61348ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePackage

## Description
`DeletePackage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/delete_package.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeletePackage` call, so each one unblocks only when `DeletePackage` works: `opensearch.delete-package-while-associated-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/delete_package.rs`. Replaying the corpus, the emulator refused the `DeletePackage` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_opensearch_delete_package_while_associated_rejected.py", line 45, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException deleting an associated package, got 'NotImplemented' ('operation DeletePackage is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the opensearch emulator
Call DeletePackage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
