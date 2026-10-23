Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qapps.concurrent-library-item-reviews-single-winner
Issue-Shape-Id: com.amazonaws.qapps#QAppsService
Issue-Key: qapps.concurrent-library-item-reviews-single-winner|run#7|AssertionError|assert results == [("UnauthorizedException", "Unauthorized")] * worker_count, (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# Implement qapps AssociateLibraryItemReview: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateLibraryItemReview

## Description
`AssociateLibraryItemReview` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-qapps-qappsservice/src/ops/associate_library_item_review.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AssociateLibraryItemReview` call, so each one unblocks only when `AssociateLibraryItemReview` works: `qapps.concurrent-library-item-reviews-single-winner`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-qapps-qappsservice/src/ops/associate_library_item_review.rs`. Replaying the corpus, the emulator refused the `AssociateLibraryItemReview` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_qapps_concurrent_library_item_reviews_single_winner.py", line 48, in run
    assert results == [("UnauthorizedException", "Unauthorized")] * worker_count, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected every concurrent caller to be rejected identically by the identity-center gate, got [('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the qapps emulator
Call AssociateLibraryItemReview and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
