Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-create-load-balancer-validation-and-duplicate-name
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-create-load-balancer-validation-and-duplicate-name elbclassic-health-check-configuration-round-trip-and-validation elbclassic-listener-lifecycle-duplicate-and-idempotent-delete elbclassic-load-balancer-attributes-round-trip-and-range-validation elbclassic-load-balancer-lifecycle-create-describe-delete elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: ec2|stub:CreateSecurityGroup
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ec2 CreateSecurityGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecurityGroup

## Description
`CreateSecurityGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_security_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateSecurityGroup` call, so each one unblocks only when `CreateSecurityGroup` works: `elbclassic-create-load-balancer-validation-and-duplicate-name#setup#3`, `elbclassic-health-check-configuration-round-trip-and-validation#setup#3`, `elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#setup#3`, `elbclassic-load-balancer-attributes-round-trip-and-range-validation#setup#3`, `elbclassic-load-balancer-lifecycle-create-describe-delete#setup#3`, `elbclassic-subnet-attach-detach-and-security-group-reassignment#setup#5`, `elbclassic-subnet-attach-detach-and-security-group-reassignment#setup#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_security_group.rs`. Replaying the corpus, the emulator refused the `CreateSecurityGroup` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call CreateSecurityGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
