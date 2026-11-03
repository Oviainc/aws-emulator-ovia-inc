Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.recommender-schema-lifecycle
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.recommender-schema-lifecycle|run#10|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}: {message!r}"
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Implement customer-profiles CreateRecommenderSchema: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRecommenderSchema

## Description
`CreateRecommenderSchema` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/create_recommender_schema.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateRecommenderSchema` call, so each one unblocks only when `CreateRecommenderSchema` works: `customer-profiles.recommender-schema-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/create_recommender_schema.rs`. Replaying the corpus, the emulator refused the `CreateRecommenderSchema` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_customer_profiles_recommender_schema_lifecycle.py", line 64, in run
    assert code == "BadRequestException", f"expected BadRequestException, got {code!r}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected BadRequestException, got 'NotImplemented': 'operation CreateRecommenderSchema is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the customer-profiles emulator
Call CreateRecommenderSchema and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
