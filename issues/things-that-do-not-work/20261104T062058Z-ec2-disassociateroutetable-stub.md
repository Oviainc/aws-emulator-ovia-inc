Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 ec2-lsrno-traj-route-lookup-implicit-main-association ec2-pst-traj-detached-igw-leaves-blackhole-default-route ec2-pst-traj-implicit-main-association-is-invisible-to-subnet-filter ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-pst-traj-wrong-kind-and-short-network-ids-are-not-found ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap ec2-subnet-route-table-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DisassociateRouteTable
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# Implement ec2 DisassociateRouteTable: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateRouteTable

## Description
`DisassociateRouteTable` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_route_table.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `DisassociateRouteTable` call, so each one unblocks only when `DisassociateRouteTable` works: `ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#teardown#12`, `ec2-lsrno-traj-route-lookup-implicit-main-association#teardown#10`, `ec2-pst-traj-detached-igw-leaves-blackhole-default-route#teardown#19`, `ec2-pst-traj-implicit-main-association-is-invisible-to-subnet-filter#teardown#14`, `ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#17`, `ec2-pst-traj-wrong-kind-and-short-network-ids-are-not-found#teardown#12`, `ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#teardown#32`, `ec2-subnet-route-table-lifecycle#task#17`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_route_table.rs`. Replaying the corpus, the emulator refused the `DisassociateRouteTable` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call DisassociateRouteTable and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
