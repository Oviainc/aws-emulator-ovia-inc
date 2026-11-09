Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_collect#9|Timeout|got = sqs.receive_message(
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# elbv2__lambda: a malformed Lambda-target response never becomes a load-balancer-generated 502 after the invocation (1 cause)

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Cause 1 of 1 — the ALB→Lambda forward path serves nothing, so a Lambda target that breaks the ALB response contract is never answered with a load-balancer-generated HTTP 502, and no invocation is ever recorded through the load balancer. Failing check id: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation.

AWS: Smithy — RegisterTargets accepts a Lambda function ARN for a `lambda`-type target group; the response object a function must return (`statusCode`, `statusDescription`, `headers`, `body`, `isBase64Encoded`) is recorded behavior only. Recorded behavior: when the function returns a payload that breaks the contract — missing `statusCode`, a non-JSON/non-ALB payload, a body above the documented 1 MB ALB response limit, or an unhandled exception — the load balancer answers the CLIENT with HTTP 502 and a body the load balancer itself generates; the malformed payload is NEVER proxied through. The function IS invoked each time (the side channel records exactly one invocation per attempt, tagged with that attempt's mode), so the 502 is the load balancer rejecting the RESPONSE, not declining to invoke. A contract violation fails only that request: DescribeTargetHealth still lists the single registered Lambda target, and the next well-formed request returns 200 with the target's own body.

Emulator: the check exhausts the harness budget — `Check timed out after 60.0s`. The control leg already fails: the ALB never returns the well-formed (200, target body) response for a function that answers correctly on a direct `lambda:Invoke`. So none of the four malformed modes (`nostatus`, `nonjson`, `toolarge`, `raise`) is ever answered 502, no invocation reaches the side channel through the load balancer, and the post-failure recovery is never observed.

Why: a client that fronts a Lambda function with an ALB cannot distinguish a malformed response (AWS: 502, function ran, target stays in service) from a function that was never invoked or a target taken out of service. Error-handling and alarming built on the 502-after-invocation contract is untestable, and a malformed payload risks being proxied to clients instead of being replaced by the load balancer's own 502 body.

Fix: the `forward`-to-`lambda`-target path of the ELBv2 data plane (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service/data_plane.rs): invoke the registered function in-process, then validate the returned payload against the ALB response contract and answer 502 with a load-balancer-generated body when it is missing `statusCode`, is not a JSON ALB response object, is oversized, or the invocation raised — without proxying the payload and without changing the target's registration or health.

Traces: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation. This restates the single cause of the open issue 20261105T093535Z-elbv2-lambda-a-lambda-target-that-breaks-the.md with the check's CURRENT replay key: that issue's key (`_collect#9|Timeout|assert len(bodies) >= want`) no longer matches — the check now fails as a bare 60s timeout — so a fix gated on that issue alone would not re-replay this check. Close the two together.

## Evidence
Failing check under `### elbv2__lambda`, not marked ALREADY FILED or OWNED:

- [mismatch] elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation — Error: Check timed out after 60.0s.

Expected (the check's assertions): a direct `lambda:Invoke` control proves the function and its SQS side channel work (side channel carries mode `ok`); the ALB control request returns exactly (200, "TARGET-OK"); then for each of `nostatus`, `nonjson`, `toolarge`, `raise` the ALB answers HTTP 502 with a body that is neither "TARGET-OK" nor the malformed payload, and the side channel records exactly one invocation for that mode; afterwards DescribeTargetHealth still lists the registered Lambda ARN and the next well-formed request returns (200, "TARGET-OK").

Actual: the ALB never answers the well-formed control request, so no 502 is ever produced and no invocation reaches the side channel through the load balancer.

## Steps to Reproduce
1. Create an SQS side-channel queue, a public VPC, a security group open on 80, an IAM role, and a Lambda function that reports each invocation to the queue and, by request path, returns either a well-formed ALB response (200, "TARGET-OK") or a malformed one: no `statusCode` (`/nostatus`), a non-JSON payload (`/nonjson`), a ~1.5 MB body (`/toolarge`), or raises (`/raise`).
2. Direct `lambda:Invoke` once and confirm the side channel carries mode `ok`; drain the queue.
3. CreateLoadBalancer (application, internet-facing), wait active; CreateTargetGroup with `TargetType=lambda`; grant elasticloadbalancing invoke permission; RegisterTargets with the function ARN; CreateListener HTTP:80 forwarding to the group.
4. GET `/ok` through the load balancer: expect (200, "TARGET-OK").
5. GET `/nostatus`, `/nonjson`, `/toolarge`, `/raise`: expect HTTP 502 each time with a load-balancer-generated body and exactly one side-channel invocation per request.
6. DescribeTargetHealth: expect the Lambda target still registered; GET `/ok` again: expect (200, "TARGET-OK").
Replay: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation
