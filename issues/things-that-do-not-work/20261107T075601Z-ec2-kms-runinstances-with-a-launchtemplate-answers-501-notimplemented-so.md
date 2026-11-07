Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__kms.asglf-kms-key-without-encrypted-flag-rejected-at-launch ec2__kms.asglf-run-instances-under-disabled-key-blocked
Issue-Key: ec2__kms.asglf-kms-key-without-encrypted-flag-rejected-at-launch|run#4|AssertionError|assert got == (_ERR_PARAM_DEPENDENCY, _MSG_PARAM_DEPENDENCY), (
Issue-Key: ec2__kms.asglf-run-instances-under-disabled-key-blocked|run#10|AssertionError|assert code == "DryRunOperation", f"DryRun under a disabled key returned {code!r}"
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# RunInstances with a LaunchTemplate answers 501 NotImplemented, so the KMS key-state refusals are unreachable

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause.

Cause 1 of 1: the RunInstances-with-a-LaunchTemplate variant is an unimplemented scaffold, so every KMS-gated launch refusal answers NotImplemented instead of the modelled error.
AWS: RunInstances(LaunchTemplate={...}) is a first-class shape; the template's block-device mapping decides the volume encryption, and the launch is refused with EC2's parameter error when a KmsKeyId is given without Encrypted=true, and with the KMS key-state failure when the key is disabled (recorded behavior; the refusal is produced even under DryRun=True, because parameter validation precedes the dry-run short-circuit).
Emulator: both calls answer ('NotImplemented', 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator') — a 501-class error for a modelled request shape, so neither refusal can be observed.
Why: launching from a launch template is the normal path for every ASG/CFN/CDK-shaped workload; with it unimplemented, no KMS key-state or encryption-flag validation is reachable and a client gets "not implemented" where AWS returns a 400 with a specific code.
Fix: the RunInstances handler's LaunchTemplate variant (services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — template resolution into the launch path) and the KMS/encryption validation it must then run. A sibling scaffold issue exists for this variant under the ec2 service heading (20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md); these two checks are listed as live failures under ec2__kms and are recorded here so the KMS refusals are re-replayed once the variant works.
Traces: ec2__kms.asglf-kms-key-without-encrypted-flag-rejected-at-launch, ec2__kms.asglf-run-instances-under-disabled-key-blocked.

## Evidence
From the behavioral replay:
- asglf-kms-key-without-encrypted-flag-rejected-at-launch [mismatch]: AssertionError: Encrypted=false, DryRun=True: RunInstances returned ('NotImplemented', 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator').
- asglf-run-instances-under-disabled-key-blocked [mismatch]: AssertionError: DryRun under a disabled key returned 'NotImplemented'.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids: create a KMS key, a launch template whose root mapping names that KmsKeyId (once with Encrypted omitted/false, once with the key disabled), then call RunInstances with LaunchTemplate and DryRun=True.
