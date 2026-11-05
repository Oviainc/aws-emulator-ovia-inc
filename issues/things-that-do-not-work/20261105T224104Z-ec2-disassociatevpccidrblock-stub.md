Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.disassociate-vpc-cidr-block-primary-rejected ec2.vpc-secondary-cidr-block-association-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DisassociateVpcCidrBlock|NotImplemented
Issue-Commit: a983232b5da502bccde79d757bd972103fe44705

# Implement ec2 DisassociateVpcCidrBlock: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a983232b5da502bccde79d757bd972103fe44705`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateVpcCidrBlock

## Description
`DisassociateVpcCidrBlock` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_vpc_cidr_block.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DisassociateVpcCidrBlock` call, so each one unblocks only when `DisassociateVpcCidrBlock` works: `ec2.disassociate-vpc-cidr-block-primary-rejected`, `ec2.vpc-secondary-cidr-block-association-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_vpc_cidr_block.rs`. Replaying the corpus, the emulator refused the `DisassociateVpcCidrBlock` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ec2_disassociate_vpc_cidr_block_primary_rejected.py", line 56, in run
    assert code == ERR_NOT_PERMITTED, f"expected {ERR_NOT_PERMITTED} rejecting the primary CIDR block, got {code!r} ({message!r})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected OperationNotPermitted rejecting the primary CIDR block, got 'NotImplemented' ('operation DisassociateVpcCidrBlock is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call DisassociateVpcCidrBlock and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
