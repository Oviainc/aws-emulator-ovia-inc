Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected
Issue-Key: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected|run#7|AssertionError|assert msg == "Health check port '70000' must be within '1-65535' or 'traffic-port'", f"health-check port 70000 refusal message: {msg!r}"
Issue-Commit: ef659389d11311dc27a7ee58cb593d22d3f663cf

# ELBv2 ModifyTargetGroup: the out-of-range HealthCheckPort refusal uses a different message template than AWS

> _Note: this issue was identified against the code base at commit `ef659389d11311dc27a7ee58cb593d22d3f663cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTargetGroup

## Description
One net-new root cause, grounded in a failing behavioral check. The HTTP status and the error code are correct; the refusal MESSAGE does not follow AWS's template.

Cause 1 of 1: ModifyTargetGroup with HealthCheckPort outside 1-65535 is refused with the right code but the wrong message template.
AWS: recorded behavior only for the code and the text - Smithy (elasticloadbalancingv2 ModifyTargetGroup) models HealthCheckPort as the string shape HealthCheckPort with NO range constraint, and the operation's only modelled errors are InvalidConfigurationRequestException (awsQueryError InvalidConfigurationRequest) and TargetGroupNotFoundException, so the 1-65535-or-'traffic-port' rule is the service's and the refusal arrives as the Query-framework code ValidationError. The recording shows HTTP 400, code ValidationError, and a message BUILT FROM THE REQUEST: "Health check port '70000' must be within '1-65535' or 'traffic-port'" - the rejected value in single quotes, then the allowed range and the 'traffic-port' literal, in that order.
Emulator: same 400 / ValidationError and the same rejected value echoed, but a re-worded template: "Health check port '70000' must be 'traffic-port' or an integer from 1 to 65535".
Why: ValidationError is the one code ELBv2 uses for every service-side input rule on this operation, so the message is what tells a caller WHICH rule it broke; a client or diagnostic matching AWS's health-check-port template does not recognise the emulator's wording and cannot attribute the refusal.
Fix: the HealthCheckPort validation error construction in the ModifyTargetGroup handler of the ELBv2 service (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/modify_target_group.rs) - emit AWS's template.
Note (not filed as a defect, unverified): the check aborts at this assertion, so its two remaining claims - that the refused modify leaves the stored HealthCheckPort at '4000', and that ModifyTargetGroup(HealthCheckPort='traffic-port') is accepted and round-trips as that literal - were never evaluated against the emulator.
Traces: ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected.

## Evidence
Check ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected, mismatch: the code assertion (ValidationError) passed; AssertionError on the message assertion - expected "Health check port '70000' must be within '1-65535' or 'traffic-port'", got "Health check port '70000' must be 'traffic-port' or an integer from 1 to 65535". Call: ModifyTargetGroup(TargetGroupArn=<a TCP:44300 instance target group with an HTTP health check on port 4000>, HealthCheckPort='70000').
The target-group name/arn and the port 70000 are from one run; the requirements are the HTTP 400, the error code ValidationError, and the message RULE (rejected value echoed in single quotes, then the '1-65535' range and the 'traffic-port' literal, in AWS's order).

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Create a VPC and a subnet, then CreateTargetGroup(Protocol='TCP', Port=44300, TargetType='instance', HealthCheckProtocol='HTTP', HealthCheckPort='4000', HealthCheckPath='/health-check').
3. ModifyTargetGroup(TargetGroupArn=<arn>, HealthCheckPort='70000') -> expect 400 ValidationError with message "Health check port '70000' must be within '1-65535' or 'traffic-port'"; observe the emulator's re-worded template.
4. Then DescribeTargetGroups on the same arn (should still report HealthCheckPort '4000') and ModifyTargetGroup(HealthCheckPort='traffic-port') (should be accepted and round-trip) - both unreached in this run.
