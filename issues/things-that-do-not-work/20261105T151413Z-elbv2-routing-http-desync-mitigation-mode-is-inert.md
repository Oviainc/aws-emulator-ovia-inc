Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_collect#9|Timeout|got = sqs.receive_message(
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# elbv2: routing.http.desync_mitigation_mode is inert — an ambiguously framed request is never answered 400 by the load balancer (1 cause)

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Cause 1 of 1 — the load-balancer attribute `routing.http.desync_mitigation_mode` has no effect on the request path, and the HTTP listener never serves a request to its Lambda target at all, so the `strictest` refusal of an ambiguously framed request is never produced. Failing check id: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target.

AWS: Smithy — LoadBalancerAttribute.Key documents `routing.http.desync_mitigation_mode`: "Determines how the load balancer handles requests that might pose a security risk to your application. The possible values are monitor, defensive, and strictest. The default is defensive." Recorded behavior: with the attribute set to `monitor`, a POST carrying BOTH `Content-Length` and `Transfer-Encoding: chunked` (the canonical smuggling vector) is FORWARDED and the target's own response is returned (200 with the target body), and the side channel records exactly one invocation; with the attribute set to `strictest`, the identical bytes are answered by the load balancer itself with the status line `HTTP/1.1 400 Bad Request`, a body the load balancer generates, and ZERO invocations of the target, while a compliant POST on the same listener in the same instant still returns 200 with the target body; restoring `monitor` makes the ambiguous request succeed again.

Emulator: the check exhausts the harness budget — `Check timed out after 60.0s` — in the first leg, waiting for the listener to forward even a compliant raw POST to its registered Lambda target. Nothing on the request path distinguishes ambiguous from compliant framing, and the stored attribute changes nothing about what the listener answers.

Why: a client relying on AWS's desync mitigation sees request-smuggling protection as configured-but-inert: a request AWS refuses with 400 before the target runs is either forwarded or never answered, so `strictest` is indistinguishable from `monitor` and security tests of the mode hang or pass vacuously.

Fix: the ELBv2 HTTP data plane (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service/data_plane.rs) — serve the forward action to a `lambda` target, and validate request framing against the load balancer's stored `routing.http.desync_mitigation_mode`, answering `400 Bad Request` from the load balancer (never invoking the target) for ambiguously framed requests under `strictest`.

Traces: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target. This restates cause 1 of the open issue 20261105T093535Z-elbv2-4-net-new-data-plane-causes-desync.md with the check's CURRENT replay key: that issue's key for this check (`_collect#9|Timeout|assert len(bodies) >= want`) no longer matches — the check now fails as a bare 60s timeout — so a fix gated on that issue alone would not re-replay this check. Close the two together.

## Evidence
Failing check under `### elbv2`, not marked ALREADY FILED or OWNED:

- [mismatch] elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target — Error: Check timed out after 60.0s.

Expected: under `routing.http.desync_mitigation_mode=monitor` the ambiguously framed POST (both `Content-Length: 8` and `Transfer-Encoding: chunked`) is forwarded and returns the target's body with 200, with exactly one invocation on the SQS side channel; under `strictest` the identical bytes get the status line `HTTP/1.1 400 Bad Request` from the load balancer with zero invocations, while a compliant POST still returns 200 + target body; restoring `monitor` makes the ambiguous POST succeed again.

Actual: the first leg (a compliant raw POST reaching the Lambda target through the listener) never succeeds, so no framing decision is ever observed and the check ends on the harness budget. Model citation: LoadBalancerAttribute.Key documents monitor|defensive|strictest with default defensive.

## Steps to Reproduce
1. Create an SQS queue as a side channel, a public VPC, a security group open on 80, an IAM role, and a Lambda function that reports each invocation to the queue and returns a well-formed ALB response.
2. Direct `lambda:Invoke` the function once and confirm the side channel carries the request (control).
3. CreateLoadBalancer (application, internet-facing), a `lambda` target group, RegisterTargets with the function ARN, grant elasticloadbalancing invoke permission, CreateListener HTTP:80 forwarding to the group.
4. ModifyLoadBalancerAttributes `routing.http.desync_mitigation_mode=monitor`; send a raw POST with BOTH `Content-Length` and `Transfer-Encoding: chunked`: expect 200 + target body and one side-channel invocation.
5. Set the attribute to `strictest` and send the identical bytes: expect `HTTP/1.1 400 Bad Request` from the load balancer and ZERO invocations, while a compliant POST still returns 200 + target body.
6. Set it back to `monitor` and expect the ambiguous POST forwarded again.
Replay: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target
