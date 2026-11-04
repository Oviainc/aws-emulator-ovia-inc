Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-route-table
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-route-table ec2-pst-traj-detached-igw-leaves-blackhole-default-route ec2-subnet-route-table-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteRoute
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# Implement ec2 DeleteRoute: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRoute

## Description
`DeleteRoute` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_route.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteRoute` call, so each one unblocks only when `DeleteRoute` works: `ec2-conflict-route-table#task#8`, `ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#17`, `ec2-subnet-route-table-lifecycle#task#15`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_route.rs`. Replaying the corpus, the emulator refused the `DeleteRoute` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidRoute.NotFound', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected 'no route with destination-cidr-block 203.0.113.0/24 in route table rtb-00000000000000041', got 'operation DeleteRoute is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteRoute and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
