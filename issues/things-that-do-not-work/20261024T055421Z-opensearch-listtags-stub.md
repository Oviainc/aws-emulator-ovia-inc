Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.list-tags-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.list-tags-nonexistent-arn-not-found|run#5|AssertionError|assert got == expected, f'ListTags: expected {expected}, got {got}'
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement opensearch ListTags: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTags

## Description
`ListTags` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/list_tags.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListTags` call, so each one unblocks only when `ListTags` works: `opensearch.list-tags-nonexistent-arn-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/list_tags.rs`. Replaying the corpus, the emulator refused the `ListTags` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_opensearch_list_tags_nonexistent_arn_not_found.py", line 35, in run
    assert got == expected, f'ListTags: expected {expected}, got {got}'
           ^^^^^^^^^^^^^^^
AssertionError: ListTags: expected ('ValidationException', 'Invalid ARN. Domain not found.'), got ('NotImplemented', 'operation ListTags is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the opensearch emulator
Call ListTags and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
