Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.search-vectors-resource-not-found-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.search-vectors-resource-not-found-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# Implement dynamodb SearchVectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchVectors

## Description
`SearchVectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/search_vectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SearchVectors` call, so each one unblocks only when `SearchVectors` works: `dynamodb.search-vectors-resource-not-found-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/search_vectors.rs`. Replaying the corpus, the emulator refused the `SearchVectors` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_dynamodb_search_vectors_resource_not_found_rejected.py", line 43, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: operation SearchVectors is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the dynamodb emulator
Call SearchVectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
