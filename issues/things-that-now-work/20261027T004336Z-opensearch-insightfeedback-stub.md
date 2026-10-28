Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.insight-lifecycle-and-feedback
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.insight-lifecycle-and-feedback|run#8|AssertionError|assert got_domain == ("ResourceNotFoundException", "Resource not found"), (
Issue-Commit: 88eb145164ed6e7175261f755ebef46622c173d0

# Implement opensearch InsightFeedback: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `88eb145164ed6e7175261f755ebef46622c173d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InsightFeedback

## Description
`InsightFeedback` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/insight_feedback.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `InsightFeedback` call, so each one unblocks only when `InsightFeedback` works: `opensearch.insight-lifecycle-and-feedback`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/insight_feedback.rs`. Replaying the corpus, the emulator refused the `InsightFeedback` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_opensearch_insight_lifecycle_and_feedback.py", line 102, in run
    assert got_domain == ("ResourceNotFoundException", "Resource not found"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException 'Resource not found' for feedback against a nonexistent domain, got ('NotImplemented', 'operation InsightFeedback is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the opensearch emulator
Call InsightFeedback and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
