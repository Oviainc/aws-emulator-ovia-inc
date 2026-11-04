Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected
Issue-Key: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: 637c0a54c7e9b591effc4af895837fd7874d2d9c

# autoscaling__ec2__iam__ssm: an ASG instance never reaches SSM-managed state, so the StartSession permission-chain scenario never settles (1 cause)

> _Note: this issue was identified against the code base at commit `637c0a54c7e9b591effc4af895837fd7874d2d9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSession

## Description
Cause 1 of 1: the SSM permission-chain scenario over an Auto Scaling group never reaches a terminal state — the barrier it waits on (one ASG instance whose instance-profile role carries AmazonSSMManagedInstanceCore registering with SSM as a managed instance, Online, and accepting StartSession) is never satisfied, so the negative half (the sibling instance without that policy being refused) is never evaluated. Failing check id: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected.

AWS: an instance launched by an Auto Scaling group with an instance profile whose role has AmazonSSMManagedInstanceCore appears in `ssm describe-instance-information` with `PingStatus=Online` and accepts `ssm start-session`; an otherwise identical instance whose role lacks that policy never registers, and StartSession against it is refused (TargetNotConnected). The recording reaches the positive barrier inside the check's own deadlines (the check allows 480s for scale-out, registration and drain). Recorded behavior only — the model describes the operations, not the registration timing.

Emulator: the check ends `Check timed out after 60.0s`; the barrier is never observed, so the emulator produced no convergence in that window rather than a wrong answer. Nothing in the run indicates the instance will ever register.

Why: a client that provisions SSM-managed capacity through an ASG and then waits for the instance to come Online before running a session waits indefinitely; it gets neither a managed instance nor a refusal it can act on, and the IAM-policy distinction the scenario exists to prove (SSM-core vs no SSM-core) is unobservable.

Fix: the reconciliation that takes an ASG-launched EC2 instance with an instance profile and registers it as an SSM managed instance (the SSM instance-information state and whatever feeds it from EC2/autoscaling launch), not the StartSession handler's own validation.

Traces: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected.

## Evidence
Expected (AWS): within the check's deadlines, `ssm describe-instance-information` reports the SSM-core instance with PingStatus Online and `ssm start-session` succeeds against it, while the instance whose role lacks AmazonSSMManagedInstanceCore stays unregistered and StartSession against it is refused.
Actual (emulator): `Error: Check timed out after 60.0s` — the positive barrier is never reached, so neither the success nor the refusal half is produced. (The 60s figure is the harness budget for this run and is not itself the requirement; the divergence is that no convergence occurs.)

## Steps to Reproduce
1. Create two identical ASG-launched instances, one with an instance profile whose role has AmazonSSMManagedInstanceCore and one without.
2. Poll `ssm describe-instance-information` for the first instance to appear with PingStatus=Online.
3. Observe the poll never succeeds. Replay autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected.
