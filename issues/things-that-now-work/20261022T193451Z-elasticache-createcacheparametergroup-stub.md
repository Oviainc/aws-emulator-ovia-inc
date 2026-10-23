Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-cluster-empty-vs-absent-description-preserved elasticache.cache-cluster-empty-vs-absent-tag-value-preserved elasticache.cache-parameter-group-lifecycle elasticache.cache-parameter-invalid-value-rejected elasticache.cache-parameter-numeric-precision-round-trip elasticache.cache-parameter-numeric-value-precision-preserved elasticache.concurrent-duplicate-parameter-group elasticache.modify-and-reset-cache-parameter-group elasticache.modify-cache-cluster-pending-vs-immediate-application elasticache.modify-cache-parameter-invalid-value-rejected elasticache.parameter-group-not-found-semantics elasticache.tag-key-reserved-prefix-and-length-rejected elasticache.tag-unicode-value-round-trip elasticache.tags-add-list-remove elasticache.tags-invalid-arn elasticache.tags-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateCacheParameterGroup|NotImplemented
Issue-Key: elasticache.concurrent-duplicate-parameter-group|run#8|AssertionError|assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {results}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement elasticache CreateCacheParameterGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheParameterGroup

## Description
`CreateCacheParameterGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_cache_parameter_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `CreateCacheParameterGroup` call, so each one unblocks only when `CreateCacheParameterGroup` works: `elasticache.cache-cluster-empty-vs-absent-description-preserved`, `elasticache.cache-cluster-empty-vs-absent-tag-value-preserved`, `elasticache.cache-parameter-group-lifecycle`, `elasticache.cache-parameter-invalid-value-rejected`, `elasticache.cache-parameter-numeric-precision-round-trip`, `elasticache.cache-parameter-numeric-value-precision-preserved`, `elasticache.concurrent-duplicate-parameter-group`, `elasticache.modify-and-reset-cache-parameter-group`, `elasticache.modify-cache-cluster-pending-vs-immediate-application`, `elasticache.modify-cache-parameter-invalid-value-rejected`, `elasticache.parameter-group-not-found-semantics`, `elasticache.tag-key-reserved-prefix-and-length-rejected`, `elasticache.tag-unicode-value-round-trip`, `elasticache.tags-add-list-remove`, `elasticache.tags-invalid-arn`, `elasticache.tags-unicode-value-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_cache_parameter_group.rs`. Replaying the corpus, the emulator refused the `CreateCacheParameterGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateCacheParameterGroup operation: operation CreateCacheParameterGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticache emulator
Call CreateCacheParameterGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
