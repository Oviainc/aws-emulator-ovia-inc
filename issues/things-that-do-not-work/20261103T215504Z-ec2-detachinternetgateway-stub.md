Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Step: teardown#114
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: ec2|stub:DetachInternetGateway
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# Implement ec2 DetachInternetGateway: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachInternetGateway

## Description
`DetachInternetGateway` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_internet_gateway.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DetachInternetGateway` call, so each one unblocks only when `DetachInternetGateway` works: `elasticloadbalancing-alb-lifecycle-register-targets#teardown#114`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_internet_gateway.rs`. Replaying the corpus, the emulator refused the `DetachInternetGateway` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call DetachInternetGateway and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
