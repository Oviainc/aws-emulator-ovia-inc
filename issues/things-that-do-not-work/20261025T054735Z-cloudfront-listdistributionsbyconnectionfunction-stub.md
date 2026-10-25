Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-distributions-by-connection-function-filters-correctly cloudfront.list-distributions-by-connection-function-index-consistency
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|ListDistributionsByConnectionFunction|NotImplemented
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# Implement cloudfront ListDistributionsByConnectionFunction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDistributionsByConnectionFunction

## Description
`ListDistributionsByConnectionFunction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distributions_by_connection_function.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListDistributionsByConnectionFunction` call, so each one unblocks only when `ListDistributionsByConnectionFunction` works: `cloudfront.list-distributions-by-connection-function-filters-correctly`, `cloudfront.list-distributions-by-connection-function-index-consistency`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distributions_by_connection_function.rs`. Replaying the corpus, the emulator refused the `ListDistributionsByConnectionFunction` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDistributionsByConnectionFunction operation: operation ListDistributionsByConnectionFunction is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call ListDistributionsByConnectionFunction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
