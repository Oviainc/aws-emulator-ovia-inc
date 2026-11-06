Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# elbv2__lambda: a malformed Lambda target response never produces its 502 at the load balancer

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
One root cause behind 1 failing check.

Cause 1 of 1: an Application Load Balancer with a Lambda target whose handler returns a response the ALB cannot interpret never answers the client, so the 502 AWS produces is unobservable.
AWS: when a Lambda target returns a payload that is not a valid ALB response (missing/invalid `statusCode`, or a non-JSON body), the load balancer answers the viewer HTTP `502 Bad Gateway` — and it does so AFTER the function has run, i.e. the invocation is recorded in the function's metrics/logs. The 502 arrives within the target group's response timeout. Model: elasticloadbalancingv2 RegisterTargets/CreateTargetGroup with TargetType `lambda` are modelled in services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10; the 502-on-malformed-response rule is recorded behavior only.
Emulator: `Error: Check timed outae after 60.0s` — the request to the listener never resolves at all, so neither the 502 nor the evidence that the function was nevertheless invoked can be observed.
Why: a client gets a hang instead of a 502; an operator cannot tell a malformed handler response from an unreachable target, and the ALB's documented behaviour of invoking the function before failing the request cannot be relied on.
Fix: the ELBv2 listener's Lambda-target invocation path — response validation and the 502 it must return. Open issue 20261104T044826Z-elbv2-lambda-target-revoking-and-regranting-the-invoke.md owns the sibling permission check; it does not carry this replay key.
Traces: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation.

## Evidence
Expected (AWS): the viewer receives HTTP 502 Bad Gateway from the listener, and the function's invocation is still recorded (it ran before the response was rejected).

Actual (emulator): elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation -> Error: Check timed out after 60.0s — no response is returned at all.

## Steps to Reproduce
1. Create an ALB, a target group with TargetType=lambda, and register a function whose handler returns a payload with no statusCode.
2. Send an HTTP request to the listener.
3. Expected: 502 Bad Gateway, with the invocation recorded. Observed: the request never resolves within the 60 s budget.
