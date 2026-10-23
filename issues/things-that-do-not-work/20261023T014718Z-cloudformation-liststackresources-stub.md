Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services cloudformation__s3__s3tables__s3vectors.cfn-s3-lookalike-s3-resource-types-do-not-make-a-stack-count cloudformation__s3__s3tables__s3vectors.cfn-s3-template-summary-and-described-resources-agree-on-variants
Issue-Key: cloudformation|ListStackResources|NotImplemented
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# Implement cloudformation ListStackResources: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
`ListStackResources` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_resources.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListStackResources` call, so each one unblocks only when `ListStackResources` works: `cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services`, `cloudformation__s3__s3tables__s3vectors.cfn-s3-lookalike-s3-resource-types-do-not-make-a-stack-count`, `cloudformation__s3__s3tables__s3vectors.cfn-s3-template-summary-and-described-resources-agree-on-variants`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_resources.rs`. Replaying the corpus, the emulator refused the `ListStackResources` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListStackResources operation: operation ListStackResources is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudformation emulator
Call ListStackResources and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
