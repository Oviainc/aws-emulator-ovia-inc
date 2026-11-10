Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy|_poll_until#24|Timeout|last = probe()
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# elbv2__lambda: a Lambda target whose health path answers 500 or raises is never reported unhealthy — no Target.ResponseCodeMismatch / Target.FailedHealthChecks verdict is ever produced

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: health checks on a Lambda-type target group never produce an unhealthy verdict, so the per-target health states a client reads are wrong for every failing function.
AWS: Smithy (elasticloadbalancingv2) TargetHealth.Reason documents the reason codes: "If the target state is healthy, a reason code is not provided"; for unhealthy, "Target.ResponseCodeMismatch - The health checks did not return an expected HTTP code" and "Target.FailedHealthChecks - The load balancer received an error while establishing a connection to the target or the target response was malformed". With health checks enabled on a Lambda target group, a function whose health path returns statusCode 500 goes State='unhealthy', Reason='Target.ResponseCodeMismatch', Description="Health checks failed with these codes: [500]" (fixed text with the observed code substituted); a function whose health path raises goes State='unhealthy', Reason='Target.FailedHealthChecks' (there is no HTTP code to mismatch); the sibling on a 200 path goes State='healthy' with no Reason and no Description member at all. The verdict is per target group and stable across subsequent health intervals, and the ALB still fails open for a group whose only target is unhealthy.
Emulator: polling DescribeTargetHealth over the three single-target Lambda groups never observes the (unhealthy, healthy, unhealthy) triple; the check exhausts its 600 s budget inside that poll and is killed with no verdict, i.e. the failing Lambda targets are never reported unhealthy and no Reason/Description is ever attached. Open issue 20261109T153103Z-elbv2-lambda-2-net-new-causes-the-alb.md records that every Lambda target is reported healthy regardless; it does not carry this check's replay key, so this check is filed here and that issue is the one to close it with if they are fixed together.
Why: a client cannot see that a Lambda target is failing: dashboards and deployment gates that read DescribeTargetHealth see a healthy fleet, HealthyHostCount-style conditions never trip, and a broken function is indistinguishable from a working one until users see errors. The missing Reason/Description also removes the only signal that says whether the function answered the wrong code or faulted.
Fix: the ELBv2 health-check evaluator for Lambda-type target groups and the target-health state DescribeTargetHealth serves — it must invoke the configured health path, classify a non-matching status as Target.ResponseCodeMismatch with the "Health checks failed with these codes: [<code>]" description, classify an invocation error/malformed response as Target.FailedHealthChecks, and omit Reason/Description for a healthy target.
Traces: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy.

## Evidence
Check elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy [mismatch]:
  Error: Check timed out after 600.0s
  The failing wait: _poll_until(_all, lambda r: (r[0]['State'], r[1]['State'], r[2]['State']) == ('unhealthy','healthy','unhealthy'), timeout_s=600, description='500/raise targets unhealthy and good target healthy').
  Expected (then asserted): bad target -> State 'unhealthy', Reason 'Target.ResponseCodeMismatch', Description 'Health checks failed with these codes: [500]'; raising target -> State 'unhealthy', Reason 'Target.FailedHealthChecks'; good target -> State 'healthy' with no 'Reason' member; the verdict holds across three further health intervals, and all three functions are still invoked by real traffic (/serve returns 200 'SERVED-BY <fn>' even from the unhealthy groups, which fail open).
  Actual: the triple is never observed within 600 s — no failing Lambda target is ever reported unhealthy, so no Reason/Description is ever served.

## Steps to Reproduce
1. Create a VPC with subnets and an internet-facing ALB; create three Lambda functions whose health path returns 200, returns statusCode 500, and raises, and grant elasticloadbalancing.amazonaws.com invoke permission on each.
2. Create three target-type=lambda target groups with HealthCheckEnabled=true (HealthCheckIntervalSeconds=5, a timeout below it) and register one function in each; add listener rules routing /bad/* and /err/* to the failing groups.
3. Poll elbv2 DescribeTargetHealth on each group: the 500 group never reports State 'unhealthy' with Reason 'Target.ResponseCodeMismatch' and Description 'Health checks failed with these codes: [500]', and the raising group never reports State 'unhealthy' with Reason 'Target.FailedHealthChecks'.
