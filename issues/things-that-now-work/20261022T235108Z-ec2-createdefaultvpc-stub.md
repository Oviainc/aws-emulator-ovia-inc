Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-default-vpc-contract
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-default-vpc-contract|run#6|AssertionError|assert code == "DefaultVpcAlreadyExists", (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateDefaultVpc: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDefaultVpc

## Description
`CreateDefaultVpc` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_default_vpc.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateDefaultVpc` call, so each one unblocks only when `CreateDefaultVpc` works: `ec2.create-default-vpc-contract`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_default_vpc.rs`. Replaying the corpus, the emulator refused the `CreateDefaultVpc` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ec2_create_default_vpc_contract.py", line 40, in run
    assert code == "DefaultVpcAlreadyExists", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected CreateDefaultVpc rejection code 'NotImplemented' (operation CreateDefaultVpc is scaffolded but not implemented in this emulator)

## Steps to Reproduce
Boot the ec2 emulator
Call CreateDefaultVpc and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
