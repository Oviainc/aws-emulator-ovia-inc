Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered
Issue-Key: ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered|run#13|AssertionError|assert got == [(ghost, 44300, "4000", "unused", "Target.NotRegistered")], (
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# ec2__elbv2: DescribeTargetHealth omits Target.Port when a target is queried by instance id alone, instead of defaulting it to the target group's port

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause.

Cause 1 of 1: a target-health row for an id-only query carries no Target.Port.
AWS: Smithy (com.amazonaws.elasticloadbalancingv2): DescribeTargetHealth takes Targets as a list of TargetDescription, whose Port member is documented "The port on which the target is listening" and is optional on input. Recorded behavior for the defaulting: when DescribeTargetHealth is called for an instance-type target group with Targets=[{"Id": "<instance-id>"}] and no Port, AWS answers one TargetHealthDescription per queried target in which Target.Port is filled in with the target group's own traffic port, HealthCheckPort is the group's health-check port (a string), and — for an id that was never registered — TargetHealth.State="unused" with Reason="Target.NotRegistered" rather than an error. The port values are the ones from the group under test (here the group's Port, 44300, and health-check port "4000"); they are not literal requirements, the requirement is that Target.Port echoes the group's port.
Emulator: the row is served with Target.Port absent — observed (id, Port, HealthCheckPort, State, Reason) = ('i-0123456789abcdef0', None, '4000', 'unused', 'Target.NotRegistered'). Everything else in the row matches AWS, including the preceding RegisterTargets refusal (ValidationError, "The following targets are not valid instances: '<id>'") and the empty unfiltered DescribeTargetHealth.
Why: a client that reads TargetHealthDescriptions[i]["Target"]["Port"] — the normal way to key health rows by target/port, which is how AWS always answers — gets a missing key or None and either crashes or mis-keys the row, and diffing tools report a spurious change. The related resolved issue 20261029T185946Z-elasticloadbalancing-elbv2-load-balancer-and-target-health-payloads-drop.md covered other dropped defaulted members in these payloads; it is closed and does not cover the id-only query path, so this is filed separately.
Fix: the DescribeTargetHealth handler/serializer — when a requested TargetDescription has no Port, echo it back with the target group's Port (for instance/ip target groups) before serialising the row.
Traces: ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered.

## Evidence
Replay output for ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered (line 56 of the check):
`assert got == [(ghost, 44300, "4000", "unused", "Target.NotRegistered")]`
→ `AssertionError: an unregistered id should be reported, not refused: [('i-0123456789abcdef0', None, '4000', 'unused', 'Target.NotRegistered')]`
Expected Target.Port = the target group's port; emulator returned no Port at all. The instance id is a fixed non-existent id used by the check; 44300/"4000" are the target group's configured port and health-check port.

## Steps to Reproduce
1. Create an instance-type target group in a VPC with Port=44300 and an HTTP health check on port 4000 (path /health-check).
2. Call RegisterTargets with Targets=[{"Id": "i-0123456789abcdef0"}] and confirm it is refused with ValidationError "The following targets are not valid instances: 'i-0123456789abcdef0'" and registers nothing.
3. Call DescribeTargetHealth(TargetGroupArn=<arn>, Targets=[{"Id": "i-0123456789abcdef0"}]). Expect exactly one TargetHealthDescription with Target.Port = 44300 (the group's port), HealthCheckPort = "4000", TargetHealth.State = "unused", Reason = "Target.NotRegistered".
