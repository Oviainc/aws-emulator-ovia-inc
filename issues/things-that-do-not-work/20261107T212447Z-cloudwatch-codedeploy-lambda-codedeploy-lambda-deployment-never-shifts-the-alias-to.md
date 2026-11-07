Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm
Issue-Key: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm|run#17|AssertionError|assert at_v2["FunctionVersion"] == v2, (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# CodeDeploy Lambda deployment never shifts the alias to the target version

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a CodeDeploy deployment of a Lambda alias completes without ever repointing the alias at its target version, so neither the successful shift nor the alarm-triggered rollback can be observed.

AWS: recorded behavior only. A CodeDeploy deployment group of compute platform Lambda, given an AppSpec that moves alias `<alias>` from CurrentVersion to TargetVersion, repoints the alias at the target version as the traffic shift completes; when a configured CloudWatch alarm breaches during the shift, the deployment is stopped and the alias is restored to the current version. The recording drives the un-breached (positive control) leg first and reads the alias's FunctionVersion.

Emulator: after the un-breached deployment the alias still points at the original version. Replay detail: `AssertionError: positive control: the un-breached deployment left the alias at '1', expected '2'`. Because the control never shifts, the alarm-driven stop-and-rollback leg is unobservable too.

Why: a client that uses CodeDeploy to roll a Lambda alias forward sees the deployment report progress while traffic never moves — invocations keep hitting the old version — and a rollback cannot be distinguished from a shift that never happened, so the whole safe-deploy mechanism is silently inert.

Fix: the CodeDeploy Lambda compute-platform deployment executor — the AppSpec traffic-shift step that must call Lambda UpdateAlias to the target version (and restore the current version on an alarm-triggered stop), plus the alarm evaluation that gates it.

Traces: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm.

## Evidence
Expected: after a successful (un-breached) Lambda deployment the alias's FunctionVersion is the AppSpec TargetVersion ('2').
Actual: the alias still reports FunctionVersion '1' — `AssertionError: positive control: the un-breached deployment left the alias at '1', expected '2'`.

## Steps to Reproduce
1. Publish two versions of a function and create an alias pointing at version 1.
2. Create a CodeDeploy application and deployment group with computePlatform=Lambda, optionally with a CloudWatch alarm configuration.
3. CreateDeployment with an AppSpec moving the alias from CurrentVersion 1 to TargetVersion 2 and let it complete without breaching the alarm.
4. GetAlias: FunctionVersion is still '1'.
