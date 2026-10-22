Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-deletion-protection-blocks-delete-until-cleared
Issue-Step: teardown#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-deletion-protection-blocks-delete-until-cleared autoscaling-lifecycle-hook-defaults-then-deleted-list-empty autoscaling-notification-configuration-defaults-then-deleted-list-empty autoscaling-op-contract-describe-policies autoscaling-tags-create-describe-delete autoscaling-update-group-partial-merges-and-clamps-desired-capacity autoscaling-warm-pool-defaults-then-deleted
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|stub:DeleteLaunchTemplate
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# Implement ec2 DeleteLaunchTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLaunchTemplate

## Description
`DeleteLaunchTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `DeleteLaunchTemplate` call, so each one unblocks only when `DeleteLaunchTemplate` works: `autoscaling-deletion-protection-blocks-delete-until-cleared#teardown#10`, `autoscaling-lifecycle-hook-defaults-then-deleted-list-empty#teardown#10`, `autoscaling-notification-configuration-defaults-then-deleted-list-empty#teardown#11`, `autoscaling-op-contract-describe-policies#teardown#10`, `autoscaling-tags-create-describe-delete#teardown#10`, `autoscaling-update-group-partial-merges-and-clamps-desired-capacity#teardown#10`, `autoscaling-warm-pool-defaults-then-deleted#teardown#10`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template.rs`. Replaying the corpus, the emulator refused the `DeleteLaunchTemplate` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteLaunchTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
