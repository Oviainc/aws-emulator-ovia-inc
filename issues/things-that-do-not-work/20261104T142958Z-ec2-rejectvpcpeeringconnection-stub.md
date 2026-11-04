Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-reject-vpc-peering-connection
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-reject-vpc-peering-connection|run#12|AssertionError|assert (code, message) == ("DryRunOperation", _DRY_RUN_MESSAGE), (
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# Implement ec2 RejectVpcPeeringConnection: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RejectVpcPeeringConnection

## Description
`RejectVpcPeeringConnection` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/reject_vpc_peering_connection.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RejectVpcPeeringConnection` call, so each one unblocks only when `RejectVpcPeeringConnection` works: `ec2.op-contract-reject-vpc-peering-connection`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/reject_vpc_peering_connection.rs`. Replaying the corpus, the emulator refused the `RejectVpcPeeringConnection` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ec2_op_contract_reject_vpc_peering_connection.py", line 86, in run
    assert (code, message) == ("DryRunOperation", _DRY_RUN_MESSAGE), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected DryRunOperation 'Request would have succeeded, but DryRun flag is set.', got 'NotImplemented' 'operation RejectVpcPeeringConnection is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ec2 emulator
Call RejectVpcPeeringConnection and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
