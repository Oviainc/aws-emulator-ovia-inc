Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge cloudfront.list-distributions-by-response-headers-policy-id-filters-correctly cloudfront.list-distributions-by-response-headers-policy-id-index-consistency cloudfront.list-distributions-by-response-headers-policy-id-nosuch-rejected cloudfront.op-contract-createResponseHeadersPolicy cloudfront.op-contract-deleteResponseHeadersPolicy cloudfront.op-contract-getResponseHeadersPolicy cloudfront.op-contract-getResponseHeadersPolicyConfig cloudfront.response-headers-policy-lifecycle cloudfront.server-side-security-headers-policy-enforced-on-response
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateResponseHeadersPolicy|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront CreateResponseHeadersPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResponseHeadersPolicy

## Description
`CreateResponseHeadersPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_response_headers_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `CreateResponseHeadersPolicy` call, so each one unblocks only when `CreateResponseHeadersPolicy` works: `cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern`, `cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge`, `cloudfront.list-distributions-by-response-headers-policy-id-filters-correctly`, `cloudfront.list-distributions-by-response-headers-policy-id-index-consistency`, `cloudfront.list-distributions-by-response-headers-policy-id-nosuch-rejected`, `cloudfront.op-contract-createResponseHeadersPolicy`, `cloudfront.op-contract-deleteResponseHeadersPolicy`, `cloudfront.op-contract-getResponseHeadersPolicy`, `cloudfront.op-contract-getResponseHeadersPolicyConfig`, `cloudfront.response-headers-policy-lifecycle`, `cloudfront.server-side-security-headers-policy-enforced-on-response`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_response_headers_policy.rs`. Replaying the corpus, the emulator refused the `CreateResponseHeadersPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateResponseHeadersPolicy operation: operation CreateResponseHeadersPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateResponseHeadersPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
