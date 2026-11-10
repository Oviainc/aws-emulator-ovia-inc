Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2__ssm.dtgthealth-guest-health-endpoint-decides-alb-target-rotation ec2__elbv2__ssm.dtgthealth-guest-stalled-health-response-times-out-and-withdraws-the-target
Issue-Key: ec2__elbv2__ssm.dtgthealth-guest-health-endpoint-decides-alb-target-rotation|_run_guest#14|Timeout|time.sleep(5)
Issue-Key: ec2__elbv2__ssm.dtgthealth-guest-stalled-health-response-times-out-and-withdraws-the-target|_run_guest#14|Timeout|time.sleep(5)
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# ec2__elbv2__ssm: the guest-driven ALB target-health scenarios never converge — an SSM Run Command inside a launched instance never completes, so neither target-rotation check reaches a verdict

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Cause 1 of 1: the in-guest leg of the ALB target-health scenarios never completes, so both checks are terminated at their 600 s budget without ever reaching the ELBv2 assertions.
AWS: recorded behavior only for the end-to-end flow. An instance launched from the AL2023 AMI resolved from the SSM public parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, with an instance profile carrying AmazonSSMManagedInstanceCore, becomes an Online managed node; SendCommand(DocumentName=AWS-RunShellScript) is then accepted (HTTP 200 with a Command.CommandId), and GetCommandInvocation reaches Status='Success' with the script's StandardOutputContent. With the guest's two HTTP servers up on the two ports and both registered as targets, the ALB probes /health and DescribeTargetHealth reports healthy, then — after the guest answers 503 on one port — State 'unhealthy' with Reason 'Target.ResponseCodeMismatch' and Description "Health checks failed with these codes: [503]" (Smithy, elasticloadbalancingv2 TargetHealth.Reason: "Target.ResponseCodeMismatch - The health checks did not return an expected HTTP code"; "Target.Timeout - The health check requests timed out"), and the withdrawn target receives no further traffic while its healthy sibling keeps serving.
Emulator: both checks run to their 600 s budget and are killed with 'Check timed out after 600.0s' — no phase after the guest step ever reports. The waits that can consume that budget are, in order, the load balancer reaching 'active' (600 s), the instance reaching 'running' (600 s), and the SSM Run Command leg (ready 600 s / run 420 s); the suite's other ELBv2 checks show load balancers reaching active, and many already-filed issues in this repo record that a launched EC2 instance never becomes an Online SSM managed node and that SendCommand is refused, which is the same leg these two checks block on. The emulator therefore never produces the Run Command result the scenario is built on, and never produces any DescribeTargetHealth transition for it.
Why: a client cannot drive or diagnose anything inside a guest: no configuration push, no in-guest probe, no 'is my health endpoint actually answering' step. Because the call never fails, it hangs instead of erroring, so automation stalls rather than reporting a problem; and the health-verdict behaviour the scenario grades (an unhealthy target losing traffic, a timed-out probe withdrawing it) stays completely unverified.
Fix: the SSM Run Command path and the managed-node registration it depends on (SendCommand / GetCommandInvocation handlers and the instance-to-managed-node state), plus the ELBv2 target-health evaluator that must then move the target to unhealthy with Target.ResponseCodeMismatch / Target.Timeout and stop forwarding to it. Nearest open issues: 20261110T025853Z-ssm-sendcommand-refuses-a-valid-aws-runshellscript-command.md and 20261109T173225Z-ssm-run-command-is-unusable-end-to-end.md — neither carries these two checks.
Traces: ec2__elbv2__ssm.dtgthealth-guest-health-endpoint-decides-alb-target-rotation, ec2__elbv2__ssm.dtgthealth-guest-stalled-health-response-times-out-and-withdraws-the-target.

## Evidence
Both checks [mismatch] fail with: Error: Check timed out after 600.0s — no assertion was ever reached.

ec2__elbv2__ssm.dtgthealth-guest-health-endpoint-decides-alb-target-rotation: after _create_alb (poll to State.Code 'active', 600 s) and the instance reaching 'running' (600 s), it calls _run_guest(ssm, instance_id, deploy_script) = SendCommand(AWS-RunShellScript) + GetCommandInvocation polled to a terminal status, asserting Status == 'Success'. It then expects DescribeTargetHealth to report the 503-serving target State='unhealthy', Reason='Target.ResponseCodeMismatch', Description='Health checks failed with these codes: [503]' while the sibling stays 'healthy', and expects the unhealthy target's in-guest forwarded-request counter not to move.
ec2__elbv2__ssm.dtgthealth-guest-stalled-health-response-times-out-and-withdraws-the-target: same shape, with the guest stalling /health past the target group's HealthCheckTimeoutSeconds so the target must be withdrawn on a timed-out probe.

Expected: each phase answers (Run Command Success with the guest's stdout; DescribeTargetHealth transitions as above).
Actual: the run produces no verdict at all within 600 s — the emulator never completes the in-guest command leg.

## Steps to Reproduce
1. ssm GetParameter(/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64) and RunInstances a t3.micro from it with an instance profile granting AmazonSSMManagedInstanceCore, in a VPC with two subnets.
2. Create an internet-facing ALB in those subnets and wait for State.Code 'active'; create a target group with HealthCheckPath=/health, Matcher.HttpCode=200, HealthCheckIntervalSeconds=5, HealthCheckTimeoutSeconds=2, thresholds 2; register the instance twice, on port 80 and port 8080.
3. Wait for the instance state 'running', then ssm SendCommand(DocumentName='AWS-RunShellScript', InstanceIds=[id]) with a script that brings up two HTTP servers and prints their loopback /health status, and poll GetCommandInvocation to a terminal status.
4. The command never reaches Status='Success' (and no DescribeTargetHealth transition follows); the scenario hangs until the 600 s budget kills it.
