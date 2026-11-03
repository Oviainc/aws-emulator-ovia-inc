Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-trajwire-read-side-mistakes
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-trajwire-read-side-mistakes ec2-ipam-trajwire-reference-path-regional-child-pool ec2-ipam-trajwire-teardown-order-mistakes ec2-ipam-trajwire-vpc-from-top-pool-then-child
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:GetIpamPoolAllocations
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# Implement ec2 GetIpamPoolAllocations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIpamPoolAllocations

## Description
`GetIpamPoolAllocations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_ipam_pool_allocations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `GetIpamPoolAllocations` call, so each one unblocks only when `GetIpamPoolAllocations` works: `ec2-ipam-trajwire-read-side-mistakes#task#8`, `ec2-ipam-trajwire-read-side-mistakes#task#10`, `ec2-ipam-trajwire-read-side-mistakes#task#11`, `ec2-ipam-trajwire-reference-path-regional-child-pool#task#9`, `ec2-ipam-trajwire-teardown-order-mistakes#task#17`, `ec2-ipam-trajwire-vpc-from-top-pool-then-child#task#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_ipam_pool_allocations.rs`. Replaying the corpus, the emulator refused the `GetIpamPoolAllocations` call: status: expected 200, got 501
$.GetIpamPoolAllocationsResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'ipamPoolAllocationSet': ''}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call GetIpamPoolAllocations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
