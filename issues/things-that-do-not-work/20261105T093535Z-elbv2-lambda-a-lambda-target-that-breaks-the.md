Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_collect#9|Timeout|assert len(bodies) >= want, (
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# elbv2__lambda: a Lambda target that breaks the ALB response contract does not yield a load-balancer-generated 502 after the invocation (1 cause)

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — a malformed Lambda-target response is not translated into a load-balancer-generated HTTP 502, and the invocation that produced it never happens.
AWS: Smithy: RegisterTargets accepts a Lambda function ARN for a `lambda`-type target group; the ALB→Lambda response contract (the object a function must return: statusCode, headers, body, isBase64Encoded) is recorded behavior only. Recorded behavior: when the function returns a payload that breaks that contract — no `statusCode`, a non-JSON payload, a payload above the ALB response size limit, or an unhandled exception — the load balancer answers the CLIENT with HTTP 502 and a body the load balancer itself generates; the malformed payload is NEVER proxied through to the client. The function IS invoked each time (the side channel records exactly one invocation per attempt, tagged with that attempt's mode), so the 502 is the load balancer rejecting the RESPONSE rather than declining to invoke. A contract violation fails only that one request: DescribeTargetHealth still reports the single registered Lambda target afterwards, and the next well-formed request returns 200 with the target's own body.
Emulator: the check exhausts the harness budget ("Check timed out after 60.0s"). The control leg already fails — the ALB never returns the well-formed (200, target body) response for a Lambda target that answers correctly on a direct lambda:Invoke — so none of the four malformed modes (`nostatus`, `nonjson`, `toolarge`, `raise`) is ever answered 502, no invocation is recorded on the side channel through the load balancer, and the post-failure recovery (target still registered, next request 200) is never observed.
Why: a client that fronts a Lambda function with an ALB cannot distinguish a function whose response is malformed (AWS: 502, function ran, target stays in service) from a function that was never invoked or a target that was taken out of service. Error-handling and alarm logic built on the 502-after-invocation contract is untestable, and a malformed payload could be proxied to clients instead of being replaced by the load balancer's own 502 body.
Fix: the `forward`-to-`lambda`-target path of the ELBv2 data plane (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service/data_plane.rs): invoke the registered function in-process, then VALIDATE the returned payload against the ALB response contract and answer 502 with a load-balancer-generated body when it is missing statusCode, not a JSON ALB response object, oversized, or the invocation raised — without proxying the payload and without changing the target's registration or health. The adjacent open issues 20261104T044826Z-elbv2-lambda-target-revoking-and-regranting-the-invoke.md (invoke-permission changes never settle) and 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md (cause 9, the forward path answers nothing) describe neighbouring symptoms; neither carries this check's replay key or the malformed-response 502 contract.
Traces: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation.

## Evidence
Replay of suite simparity-behavioral-aws-v1. Failing check under `### elbv2__lambda`, not marked ALREADY FILED or OWNED:

- [mismatch] elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation — Error: Check timed out after 60.0s.

Expected (from the check's assertions): a direct lambda:Invoke control proves the function and its SQS side channel work (side channel carries mode `ok`); the ALB control request returns exactly (200, "TARGET-OK"); then for each mode in (`nostatus`, `nonjson`, `toolarge`, `raise`) the ALB answers HTTP 502 with a body that is neither "TARGET-OK" nor the malformed payload (no "NO-STATUS-CODE" and no "not an ALB response object" text proxied through), and the side channel records exactly one invocation for that mode; afterwards DescribeTargetHealth still lists the single registered Lambda ARN, and the next well-formed request returns (200, "TARGET-OK") with the side channel recording mode `ok`.

Actual: the ALB never answers the well-formed control request, so no 502 is ever produced, no invocation reaches the side channel through the load balancer, and the check ends on the harness budget.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check in trace_ids.
1. Create a Lambda function that reports each invocation to an SQS side channel and, by request path, returns either a well-formed ALB response (200, body "TARGET-OK") or a malformed one: no `statusCode` (`/nostatus`), a non-JSON/non-ALB payload (`/nonjson`), an oversized payload (`/toolarge`), or an unhandled exception (`/raise`).
2. Direct lambda:Invoke the function once and confirm the side channel carries mode `ok`.
3. Create an internet-facing ALB, a `lambda` target group, grant elasticloadbalancing invoke permission, RegisterTargets with the function ARN, and CreateListener HTTP:80 forwarding to that group.
4. GET `/ok` through the load balancer: expect (200, "TARGET-OK").
5. GET `/nostatus`, `/nonjson`, `/toolarge`, `/raise`: expect HTTP 502 each time with a load-balancer-generated body (not the malformed payload, not "TARGET-OK"), and expect exactly one side-channel invocation tagged with that mode per request.
6. Call DescribeTargetHealth: expect the single registered Lambda ARN still present. GET `/ok` again: expect (200, "TARGET-OK") and one side-channel invocation with mode `ok`.
