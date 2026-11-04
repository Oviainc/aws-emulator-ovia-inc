Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.modify-vpc-encryption-control-invalid-mode-rejected ec2.op-contract-createVpcEncryptionControl ec2.op-contract-deleteVpcEncryptionControl ec2.op-contract-describe-vpc-encryption-controls ec2.op-contract-modify-vpc-encryption-control
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEncryptionControl|NotImplemented
Issue-Key: ec2.op-contract-createVpcEncryptionControl|run#6|AssertionError|assert err.get("Code") == "DryRunOperation", f"expected DryRunOperation, got {err!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateVpcEncryptionControl: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEncryptionControl

## Description
`CreateVpcEncryptionControl` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_encryption_control.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateVpcEncryptionControl` call, so each one unblocks only when `CreateVpcEncryptionControl` works: `ec2.modify-vpc-encryption-control-invalid-mode-rejected`, `ec2.op-contract-createVpcEncryptionControl`, `ec2.op-contract-deleteVpcEncryptionControl`, `ec2.op-contract-describe-vpc-encryption-controls`, `ec2.op-contract-modify-vpc-encryption-control`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_encryption_control.rs`. Replaying the corpus, the emulator refused the `CreateVpcEncryptionControl` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEncryptionControl operation: operation CreateVpcEncryptionControl is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateVpcEncryptionControl and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
