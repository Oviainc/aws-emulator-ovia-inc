Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.concurrent-update-origin-request-policy-single-winner cloudfront.delete-managed-origin-request-policy-rejected cloudfront.get-origin-request-policy-config-empty-vs-absent-lists-preserved cloudfront.list-distributions-by-origin-request-policy-id-filters-correctly cloudfront.list-distributions-by-origin-request-policy-id-index-consistency cloudfront.op-contract-createOriginRequestPolicy cloudfront.op-contract-deleteOriginRequestPolicy cloudfront.op-contract-get-origin-request-policy-config cloudfront.origin-request-policy-duplicate-name-fails cloudfront.origin-request-policy-lifecycle cloudfront.update-managed-origin-request-policy-fails cloudfront.update-origin-request-policy-concurrent-modification-detected cloudfront.update-origin-request-policy-inconsistent-quantities-fails cloudfront.update-origin-request-policy-inconsistent-quantities-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateOriginRequestPolicy|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront CreateOriginRequestPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginRequestPolicy

## Description
`CreateOriginRequestPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_origin_request_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateOriginRequestPolicy` call, so each one unblocks only when `CreateOriginRequestPolicy` works: `cloudfront.concurrent-update-origin-request-policy-single-winner`, `cloudfront.delete-managed-origin-request-policy-rejected`, `cloudfront.get-origin-request-policy-config-empty-vs-absent-lists-preserved`, `cloudfront.list-distributions-by-origin-request-policy-id-filters-correctly`, `cloudfront.list-distributions-by-origin-request-policy-id-index-consistency`, `cloudfront.op-contract-createOriginRequestPolicy`, `cloudfront.op-contract-deleteOriginRequestPolicy`, `cloudfront.op-contract-get-origin-request-policy-config`, `cloudfront.origin-request-policy-duplicate-name-fails`, `cloudfront.origin-request-policy-lifecycle`, `cloudfront.update-managed-origin-request-policy-fails`, `cloudfront.update-origin-request-policy-concurrent-modification-detected`, `cloudfront.update-origin-request-policy-inconsistent-quantities-fails`, `cloudfront.update-origin-request-policy-inconsistent-quantities-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_origin_request_policy.rs`. Replaying the corpus, the emulator refused the `CreateOriginRequestPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateOriginRequestPolicy operation: operation CreateOriginRequestPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateOriginRequestPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
