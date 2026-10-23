Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.serverless-cache-not-found-semantics
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.serverless-cache-not-found-semantics|_assert_error#5|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code} ({actual!r})"
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# Implement elasticache DeleteServerlessCache: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteServerlessCache

## Description
`DeleteServerlessCache` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_serverless_cache.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteServerlessCache` call, so each one unblocks only when `DeleteServerlessCache` works: `elasticache.serverless-cache-not-found-semantics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_serverless_cache.rs`. Replaying the corpus, the emulator refused the `DeleteServerlessCache` call: File "check_elasticache_serverless_cache_not_found_semantics.py", line 46, in run
    _assert_error(
    ~~~~~~~~~~~~~^
        lambda: ec.delete_serverless_cache(ServerlessCacheName=bogus),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        "ServerlessCacheNotFoundFault",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        not_found,
        ^^^^^^^^^^
    )
    ^
  File "check_elasticache_serverless_cache_not_found_semantics.py", line 27, in _assert_error
    assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code} ({actual!r})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ServerlessCacheNotFoundFault, got NotImplemented ('operation DeleteServerlessCache is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the elasticache emulator
Call DeleteServerlessCache and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
