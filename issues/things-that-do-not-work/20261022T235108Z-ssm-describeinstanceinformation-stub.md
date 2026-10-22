Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cli-script-cfn-update-stack-security-group-blocks-ssm-negative ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id ec2__ssm.w22n-tag-filter-mixed-with-a-platform-filter-rejected-on-both-node-apis
Issue-Key: ssm|DescribeInstanceInformation|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ssm DescribeInstanceInformation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
`DescribeInstanceInformation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DescribeInstanceInformation` call, so each one unblocks only when `DescribeInstanceInformation` works: `cloudformation__ec2__ssm.cli-script-cfn-update-stack-security-group-blocks-ssm-negative`, `ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id`, `ec2__ssm.w22n-tag-filter-mixed-with-a-platform-filter-rejected-on-both-node-apis`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs`. Replaying the corpus, the emulator refused the `DescribeInstanceInformation` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeInstanceInformation operation: operation DescribeInstanceInformation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call DescribeInstanceInformation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
