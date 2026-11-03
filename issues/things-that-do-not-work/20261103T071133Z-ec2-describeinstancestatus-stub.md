Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-byvpc-traj-stopped-instance-stays-in-its-vpc
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-byvpc-traj-stopped-instance-stays-in-its-vpc ec2-ecc-traj-instance-reference-mistakes ec2-ecc-traj-running-sweep-state-filter-mistakes ec2-trajwire-lia-absent-legacy-instance-id-not-found ec2-trajwire-lia-instance-status-route-mistakes ec2-w22n-traj-ec2-describe-instances-route ec2-xrg-traj-unfiltered-count-includes-terminated
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DescribeInstanceStatus
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# Implement ec2 DescribeInstanceStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceStatus

## Description
`DescribeInstanceStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `DescribeInstanceStatus` call, so each one unblocks only when `DescribeInstanceStatus` works: `ec2-byvpc-traj-stopped-instance-stays-in-its-vpc#task#11`, `ec2-byvpc-traj-stopped-instance-stays-in-its-vpc#task#12`, `ec2-ecc-traj-instance-reference-mistakes#task#2`, `ec2-ecc-traj-running-sweep-state-filter-mistakes#task#10`, `ec2-trajwire-lia-absent-legacy-instance-id-not-found#task#2`, `ec2-trajwire-lia-instance-status-route-mistakes#task#1`, `ec2-trajwire-lia-instance-status-route-mistakes#task#2`, `ec2-trajwire-lia-instance-status-route-mistakes#task#3`, `ec2-w22n-traj-ec2-describe-instances-route#task#4`, `ec2-w22n-traj-ec2-describe-instances-route#task#5`, `ec2-xrg-traj-unfiltered-count-includes-terminated#task#9`, `ec2-xrg-traj-unfiltered-count-includes-terminated#task#10`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_status.rs`. Replaying the corpus, the emulator refused the `DescribeInstanceStatus` call: status: expected 200, got 501
$.DescribeInstanceStatusResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'instanceStatusSet': ''}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeInstanceStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
