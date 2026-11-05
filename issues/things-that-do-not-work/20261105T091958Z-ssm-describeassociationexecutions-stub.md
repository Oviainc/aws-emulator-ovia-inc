Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.describe-association-execution-targets-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeAssociationExecutions|NotImplemented
Issue-Commit: 30d5092aa3ebb101aaddada2a4fffe08f2e86509

# Implement ssm DescribeAssociationExecutions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `30d5092aa3ebb101aaddada2a4fffe08f2e86509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssociationExecutions

## Description
`DescribeAssociationExecutions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_association_executions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeAssociationExecutions` call, so each one unblocks only when `DescribeAssociationExecutions` works: `ssm.describe-association-execution-targets-nonexistent-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_association_executions.rs`. Replaying the corpus, the emulator refused the `DescribeAssociationExecutions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeAssociationExecutions operation: operation DescribeAssociationExecutions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call DescribeAssociationExecutions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
