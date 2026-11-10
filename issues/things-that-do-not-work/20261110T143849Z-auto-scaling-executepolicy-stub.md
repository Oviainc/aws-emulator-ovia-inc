Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group
Issue-Key: auto-scaling|ExecutePolicy|NotImplemented
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# Implement auto-scaling ExecutePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecutePolicy

## Description
`ExecutePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ExecutePolicy` call, so each one unblocks only when `ExecutePolicy` works: `autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs`. Replaying the corpus, the emulator refused the `ExecutePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ExecutePolicy operation: operation ExecutePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the auto-scaling emulator
Call ExecutePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
