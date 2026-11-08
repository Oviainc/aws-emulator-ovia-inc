Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elasticache.vvd-valkey-replication-group-lives-in-the-vpc-and-pins-its-network
Issue-Key: ec2__elasticache.vvd-valkey-replication-group-lives-in-the-vpc-and-pins-its-network|run#14|AssertionError|assert got == want, f"deleting a creating group raised {got!r}, expected {want!r}"
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# Implement elasticache DeleteReplicationGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReplicationGroup

## Description
`DeleteReplicationGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_replication_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteReplicationGroup` call, so each one unblocks only when `DeleteReplicationGroup` works: `ec2__elasticache.vvd-valkey-replication-group-lives-in-the-vpc-and-pins-its-network`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_replication_group.rs`. Replaying the corpus, the emulator refused the `DeleteReplicationGroup` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ec2__elasticache_vvd_valkey_replication_group_lives_in_the_vpc_and_pins_its_network.py", line 41, in run
    assert got == want, f"deleting a creating group raised {got!r}, expected {want!r}"
           ^^^^^^^^^^^
AssertionError: deleting a creating group raised ('NotImplemented', 'operation DeleteReplicationGroup is scaffolded but not implemented in this emulator'), expected ('InvalidReplicationGroupState', 'Cache cluster s2r-vvd-c3383f531831-001 is not in a valid state to be deleted.')

## Steps to Reproduce
Boot the elasticache emulator
Call DeleteReplicationGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
