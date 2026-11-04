Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# ELBv2 Lambda target: revoking and regranting the invoke permission never settles

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Cause 1 of 1: an Application Load Balancer with a Lambda target group must answer 502 once the function's elasticloadbalancing invoke permission is removed, and answer normally again once it is regranted.
AWS: recorded behavior only — the recording registers the function as a target, proves a request succeeds, calls lambda:RemovePermission for the elasticloadbalancing.amazonaws.com statement, observes HTTP 502 from the listener, then re-adds the permission and observes success again; on AWS the whole cycle completes inside the suite's 60s budget.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — neither the denied leg nor the regranted leg ever produces a listener response, so the poll never terminates.
Why: a client cannot observe the permission's effect at all: a revoked grant produces no 502 and a restored grant produces no recovery, so an ALB-to-Lambda authorization mistake is invisible.
Fix: the ELBv2 listener's Lambda-target invocation path, including the resource-policy evaluation and the 502 it must produce when the invoke is refused.
Traces: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted.

## Evidence
Expected (AWS): the listener answers 502 while the elasticloadbalancing invoke permission is absent and answers normally after it is regranted.
Actual (emulator): 'Error: Check timed out after 60.0s' — the listener never answers on either leg.

## Steps to Reproduce
1. Create an ALB with a target group of TargetType=lambda, RegisterTargets with a function, and AddPermission for elasticloadbalancing.amazonaws.com.
2. Request the listener (succeeds), RemovePermission, request again expecting 502, AddPermission again, request again.
3. Observe the scenario never settles within 60s.
