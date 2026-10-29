Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteIpamPool
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DeleteIpamPool: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPool

## Description
`DeleteIpamPool` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_pool.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteIpamPool` call, so each one unblocks only when `DeleteIpamPool` works: `ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#14`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_pool.rs`. Replaying the corpus, the emulator refused the `DeleteIpamPool` call: status: expected 200, got 501
$.DeleteIpamPoolResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'ipamPool': {'addressFamily': 'ipv4', 'autoImport': 'false', 'ipamArn': 'arn:aws:ec2::000000000000:ipam/ipam-00000000000000023', 'ipamPoolArn': 'arn:aws:ec2::000000000000:ipam-pool/ipam-pool-00000000000000029', 'ipamPoolId': 'ipam-pool-00000000000000029', 'ipamRegion': 'us-west-2', 'ipamScopeArn': 'arn:aws:ec2::000000000000:ipam-scope/ipam-scope-00000000000000025', 'ipamScopeType': 'private', 'locale': 'None', 'ownerId': '000000000000', 'poolDepth': '1', 'state': 'delete-in-progress'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteIpamPool and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
