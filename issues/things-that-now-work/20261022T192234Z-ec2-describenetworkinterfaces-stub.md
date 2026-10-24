Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-create-load-balancer-validation-and-duplicate-name
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-create-load-balancer-validation-and-duplicate-name elbclassic-health-check-configuration-round-trip-and-validation elbclassic-listener-lifecycle-duplicate-and-idempotent-delete elbclassic-load-balancer-attributes-round-trip-and-range-validation elbclassic-load-balancer-lifecycle-create-describe-delete elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: ec2|stub:DescribeNetworkInterfaces
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ec2 DescribeNetworkInterfaces: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
`DescribeNetworkInterfaces` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `DescribeNetworkInterfaces` call, so each one unblocks only when `DescribeNetworkInterfaces` works: `elbclassic-create-load-balancer-validation-and-duplicate-name#teardown#12`, `elbclassic-health-check-configuration-round-trip-and-validation#teardown#12`, `elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#teardown#12`, `elbclassic-load-balancer-attributes-round-trip-and-range-validation#teardown#11`, `elbclassic-load-balancer-lifecycle-create-describe-delete#teardown#9`, `elbclassic-subnet-attach-detach-and-security-group-reassignment#teardown#17`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs`. Replaying the corpus, the emulator refused the `DescribeNetworkInterfaces` call: status: expected 200, got 501
$: poll ended: 501 NotImplemented cannot satisfy until $.DescribeNetworkInterfacesResponse.networkInterfaceSet in [''] after 1 attempt(s) (expected None, got {'Response': {'Errors': {'Error': {'Code': 'NotImplemented', 'Message': 'operation DescribeNetworkInterfaces is scaffolded but not implemented in this emulator'}}}})

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeNetworkInterfaces and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
