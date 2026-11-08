Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.wafv2-rule-evaluation-allows-blocks
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.wafv2-rule-evaluation-allows-blocks|run#36|Timeout|time.sleep(2)
Issue-Commit: 01ad7cf4d75cdcadee33f1733defefde67ff031f

# wafv2 data plane: a WebACL associated with an internet-facing ALB never enforces its Block rule — the rule-evaluation check never reaches a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `01ad7cf4d75cdcadee33f1733defefde67ff031f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
One net-new root cause on the wafv2 data-plane (edge enforcement) surface.

Cause 1 of 1: a REGIONAL WebACL associated with a live internet-facing Application Load Balancer is never enforced on the data path, so the check that proves Block/Allow rule evaluation at the edge never reaches a verdict.

AWS: recorded behavior only for the data path — AWS does not model the edge response, but the control-plane call that creates the binding IS modeled (Smithy: AssociateWebACL, required WebACLArn and ResourceArn, both ResourceArn @length 20..2048 @pattern \S; errors WAFNonexistentItemException, WAFUnavailableEntityException, WAFInvalidParameterException, WAFInvalidOperationException, WAFLimitsExceededException), and GetSampledRequests (required WebAclArn/RuleMetricName/Scope/TimeWindow/MaxItems) is the modeled observation point for a matched request. The recorded behavior is: once AssociateWebACL binds a REGIONAL WebACL whose DefaultAction is Allow and whose Priority-0 rule is a ByteMatchStatement on SingleHeader x-waf-block with PositionalConstraint EXACTLY, SearchString "1" and Action Block, the ALB's HTTP listener answers (a) a request WITHOUT that header with the listener's own fixed-response 200 (body "ok"), and (b) a request carrying `x-waf-block: 1` with 403 produced by WAF before the request reaches the listener's action. Both effects become observable within the check's budgets (300 s for the allowed 200, 180 s for the blocked 403) against real AWS. The 403 is WAF's own refusal; its status is the requirement, not any per-run body or request id.

Emulator: the check ends as `Error: Check timed out after 60.0s` with no assertion verdict — the emulator never produced the state the check polls for. The check's own polls are, in order: the ALB reaching State.Code "active" (describe_load_balancers, 300 s), AssociateWebACL succeeding under a bounded retry over ThrottlingException / WAFUnavailableEntityException / WAFInternalErrorException, then `GET http://<lb dns>/` returning 200, then the same GET with `x-waf-block: 1` returning 403. Since no assertion fired, the polled condition — a reachable ALB endpoint that answers the allowed request 200 and the header-matching request 403 — was never reached inside the cap. This is the ALB-fronted sibling of the API-Gateway-fronted edge hangs already filed in 20261030T002309Z-wafv2-data-plane-the-captcha-challenge-edge-response.md (Cause 2) and of the open 20261104T094656Z-elbv2-wafv2-an-alb-with-a-lambda-target.md / 20261104T094656Z-elbv2-logs-wafv2-a-webacl-associated-with-an.md, which carry other services' replay keys and none of this check's; neither covers this failure.

Why: a client that associates a WebACL with a load balancer and expects WAF to refuse matching requests with 403 while passing everything else gets no enforcement at all against the emulator — the only observable confirmation that a stored rule set DOES anything on an ALB data path. Request-blocking logic, WAF-fronted smoke tests and any integration that asserts a 403 from an associated WebACL cannot be exercised; worse, it silently looks like "allowed", so a test suite that expects denial sees traffic sail through.

Fix: the wafv2 data-plane rule-evaluation path for resources associated via AssociateWebACL, and its wiring to the elbv2 listener data path — an associated REGIONAL WebACL must be consulted for every request to the load balancer's listener, terminating with 403 on a Block-action match and otherwise deferring to the listener's own action; the association state AssociateWebACL writes must be visible to that path promptly after the call returns.

Traces: wafv2.wafv2-rule-evaluation-allows-blocks.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator: 165/209 passed; wafv2 153/190 (81%), 37 failed. Every other wafv2 failure in that report is marked OWNED by an open issue or ALREADY FILED against an unimplemented stub; this one is not.

Failing check: [mismatch] wafv2.wafv2-rule-evaluation-allows-blocks — reported only as `Error: Check timed out after 60.0s`, i.e. no assertion verdict was produced.

Expected (real AWS): after create_web_acl(Scope=REGIONAL, DefaultAction={Allow:{}}, Rules=[{Name: block-header, Priority: 0, Action: {Block:{}}, Statement: {ByteMatchStatement: {SearchString: b"1", FieldToMatch: {SingleHeader: {Name: "x-waf-block"}}, TextTransformations: [{Priority: 0, Type: NONE}], PositionalConstraint: EXACTLY}}}]) and associate_web_acl(WebACLArn=<acl arn>, ResourceArn=<alb arn>):
  * GET http://<alb dns>/ with no x-waf-block header -> HTTP 200 (the listener's fixed-response, body "ok") within 300 s;
  * GET http://<alb dns>/ with header `x-waf-block: 1` -> HTTP 403 from WAF within 180 s;
  * the allowed GET still -> 200 afterwards.
The DNS name, ARNs and ALB id are per-run values and are not part of the requirement; the 200/403 statuses and which request gets which are.

Actual (emulator): the check never completed any of these three legs — it was still inside one of its bounded polls (ALB State.Code == "active", then 200 on the allowed request, then 403 on the header-matching request) when the 60 s harness cap terminated it. No status mismatch could be recorded because no leg reached its assertion; the observable WAF effect (403 for the matching request) was never produced.

Note: the check performs no live-account-only assertion — it builds its own VPC, subnets, IGW, security group, ALB and fixed-response listener through the emulator's own ec2/elbv2 surfaces and reaches the ALB through ctx.endpoint_route, so the unreached condition is the emulator's behavior, not a runner limitation.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite (run_behavioral_suite); the check id in trace_ids reproduces this cause.
2. Manually: ec2 CreateVpc 10.99.0.0/16, an InternetGateway attached to it, a route table with 0.0.0.0/0 -> the IGW, and two subnets (10.99.0.0/24, 10.99.1.0/24) in two distinct available AZs associated with that route table; a security group in the VPC with TCP 80 ingress open to the runner's CIDRs.
3. elbv2 CreateLoadBalancer(Type=application, Scheme=internet-facing, Subnets=<the two>, SecurityGroups=[<sg>]), then CreateListener(Protocol=HTTP, Port=80, DefaultActions=[{Type: fixed-response, FixedResponseConfig: {StatusCode: "200", ContentType: text/plain, MessageBody: "ok"}}]). Poll DescribeLoadBalancers until State.Code == "active".
4. wafv2 CreateWebACL(Scope=REGIONAL, DefaultAction={Allow:{}}, VisibilityConfig with SampledRequestsEnabled and CloudWatchMetricsEnabled true, Rules=[{Name: block-header, Priority: 0, Action: {Block:{}}, Statement: {ByteMatchStatement: {SearchString: b"1", FieldToMatch: {SingleHeader: {Name: "x-waf-block"}}, TextTransformations: [{Priority: 0, Type: NONE}], PositionalConstraint: EXACTLY}}, VisibilityConfig: {...}}]), then AssociateWebACL(WebACLArn=<acl arn>, ResourceArn=<lb arn>).
5. GET http://<lb dns>/ with no extra header -> expect 200. GET the same URL with `x-waf-block: 1` -> expect 403 from WAF. GET again with no header -> expect 200.
6. Observed: the emulator never satisfies this sequence — the check polling exactly these conditions is killed by the 60 s cap without any leg asserting, so the WebACL's Block rule is never enforced on the ALB data path.
