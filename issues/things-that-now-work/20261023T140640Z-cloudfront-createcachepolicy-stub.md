Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cache-policy-invalid-ttl-order-rejected cloudfront.cache-policy-lifecycle-crud cloudfront.cache-policy-update-stale-etag-fails cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present cloudfront.cf-wildcard-invalidation-clears-only-matching-objects cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag cloudfront.cfcache-cookie-allowlist-separates-cache-entries cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache cloudfront.create_cache_policy_duplicate_name_rejected cloudfront.get-cache-policy-config-round-trip cloudfront.list-distributions-by-cache-policy-id-consistency cloudfront.managed-cache-policy-delete-rejected cloudfront.max-ttl-clamps-origin-cache-control-max-age cloudfront.op-contract-create-cache-policy cloudfront.op-contract-deleteCachePolicy cloudfront.op-contract-get-cache-policy-config cloudfront.origin-request-policy-header-does-not-enter-the-cache-key cloudfront.update-managed-cache-policy-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cache-policy-invalid-ttl-order-rejected|run#5|AssertionError|assert code == "InvalidArgument", (
Issue-Key: cloudfront|CreateCachePolicy|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront CreateCachePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCachePolicy

## Description
`CreateCachePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_cache_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `CreateCachePolicy` call, so each one unblocks only when `CreateCachePolicy` works: `cloudfront.cache-policy-invalid-ttl-order-rejected`, `cloudfront.cache-policy-lifecycle-crud`, `cloudfront.cache-policy-update-stale-etag-fails`, `cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries`, `cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present`, `cloudfront.cf-wildcard-invalidation-clears-only-matching-objects`, `cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag`, `cloudfront.cfcache-cookie-allowlist-separates-cache-entries`, `cloudfront.cfcache-min-ttl-floors-origin-no-cache-directive`, `cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache`, `cloudfront.create_cache_policy_duplicate_name_rejected`, `cloudfront.get-cache-policy-config-round-trip`, `cloudfront.list-distributions-by-cache-policy-id-consistency`, `cloudfront.managed-cache-policy-delete-rejected`, `cloudfront.max-ttl-clamps-origin-cache-control-max-age`, `cloudfront.op-contract-create-cache-policy`, `cloudfront.op-contract-deleteCachePolicy`, `cloudfront.op-contract-get-cache-policy-config`, `cloudfront.origin-request-policy-header-does-not-enter-the-cache-key`, `cloudfront.update-managed-cache-policy-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_cache_policy.rs`. Replaying the corpus, the emulator refused the `CreateCachePolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudfront_cache_policy_invalid_ttl_order_rejected.py", line 121, in run
    assert code == "InvalidArgument", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: MaxTTL(60) < MinTTL(3600) must be refused with InvalidArgument -- NOT InvalidTTLOrder, which CreateCachePolicy does not model at all (measured on real AWS); got 'NotImplemented': 'operation CreateCachePolicy is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateCachePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
