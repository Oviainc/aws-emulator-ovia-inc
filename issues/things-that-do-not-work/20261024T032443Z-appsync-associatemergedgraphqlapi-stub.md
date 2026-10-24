Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-association-operations
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-association-operations|run#11|AssertionError|assert len(successes) >= 1, f"expected at least one AssociateMergedGraphqlApi racer to win, got {results!r}"
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Implement appsync AssociateMergedGraphqlApi: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateMergedGraphqlApi

## Description
`AssociateMergedGraphqlApi` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/associate_merged_graphql_api.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AssociateMergedGraphqlApi` call, so each one unblocks only when `AssociateMergedGraphqlApi` works: `appsync.concurrent-association-operations`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/associate_merged_graphql_api.rs`. Replaying the corpus, the emulator refused the `AssociateMergedGraphqlApi` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_appsync_concurrent_association_operations.py", line 179, in run
    assert len(successes) >= 1, f"expected at least one AssociateMergedGraphqlApi racer to win, got {results!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected at least one AssociateMergedGraphqlApi racer to win, got [ClientError('An error occurred (NotImplemented) when calling the AssociateMergedGraphqlApi operation: operation AssociateMergedGraphqlApi is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the AssociateMergedGraphqlApi operation: operation AssociateMergedGraphqlApi is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the AssociateMergedGraphqlApi operation: operation AssociateMergedGraphqlApi is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the AssociateMergedGraphqlApi operation: operation AssociateMergedGraphqlApi is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the appsync emulator
Call AssociateMergedGraphqlApi and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
