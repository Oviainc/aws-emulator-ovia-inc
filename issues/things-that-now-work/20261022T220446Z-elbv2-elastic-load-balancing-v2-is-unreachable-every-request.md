Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes
Issue-Key: cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn|run#11|ResponseParserError|first = elb.create_target_group(**spec)
Issue-Key: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes|run#11|ResponseParserError|created = elb.create_target_group(
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Elastic Load Balancing v2 is unreachable: every request is refused with AmbiguousService for signing name 'elasticloadbalancing', and the refusal is rendered as a JSON body to an awsQuery client

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
TWO root causes, both reached by the two elbv2 behavioral checks in this suite. BOTH must be closed.

Cause 1 of 2: two emulated services claim the signing name `elasticloadbalancing`, so the router refuses every ELB/ELBv2 request instead of serving it.
AWS: Smithy: the ELBv2 service (`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`) and the classic ELB service (`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7`) share the sigv4 signing name `elasticloadbalancing` but are distinct APIs with distinct apiVersions (2015-12-01 vs 2012-06-01), which real AWS disambiguates by the `Version` form field / endpoint of the awsQuery request. `CreateTargetGroup` returns HTTP 200 with a CreateTargetGroupOutput whose `TargetGroups[0].TargetGroupArn` is the new (or, on an identical replay, the existing) target-group ARN; its modeled client errors are DuplicateTargetGroupName, TooManyTargetGroups, InvalidConfigurationRequest, TooManyTags. `AmbiguousService` is not a modeled error and 2 claimants is not a client-visible condition on AWS.
Emulator: every ELBv2 call is refused with the body `{"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'elasticloadbalancing': elasticloadbalancing, elasticloadbalancing; the request is rejected rather than served by an arbitrary one"}`. No ELBv2 operation can be invoked at all.
Why: the whole service is unusable — a client cannot create, describe, tag or delete a load balancer or target group, and the refusal is an unmodeled error code that no SDK error-handling branch can match. There is a sibling open issue for the same framework behaviour on another service (20261022T212829Z-migrationhub-config-wire-replay-every-operation-refused-with.md, "every operation refused with AmbiguousService"); it does not cover the `elasticloadbalancing` pair.
Fix: the two services' claim seam — `can_handle` in services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs and its v7 counterpart must disambiguate on the request's awsQuery `Version` (or `Action`) rather than on the shared signing name alone; that method's own doc names a shared signing name as exactly the case it exists for.
Traces: cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn, cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes.

Cause 2 of 2: a framework-level refusal for an awsQuery service is serialized in the awsJson envelope, so the SDK cannot parse the response at all.
AWS: Smithy: ELBv2 speaks awsQuery; every response, success or error, is XML (`<ErrorResponse><Error><Type>Sender</Type><Code>...</Code><Message>...</Message></Error><RequestId>...</RequestId></ErrorResponse>`) with content-type text/xml. Recorded behavior: botocore parses the error through its XML parser for this protocol.
Emulator: the refusal body is JSON with an awsJson `__type` discriminator (see Cause 1's body), so botocore raises `ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received` before it can read any code. The client therefore sees a transport-level parse failure, not an error code — and because botocore reports "Further retries may succeed", it may retry a request that can never succeed.
Why: even once a caller is told something went wrong, it gets no usable error code or message; any protocol-correct client of an XML service breaks on the envelope rather than on the condition. This affects every framework-level refusal on every awsQuery service, not only AmbiguousService.
Fix: the router's framework-error renderer must pick the envelope from the target service's protocol (the `override_framework_error` / default rendering path described in src/service.rs), emitting the XML `<ErrorResponse>` form for awsQuery services.
Traces: same two checks (both fail with ResponseParserError rather than with a code).

## Evidence
Both elbv2 checks fail identically:
`botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:`
`b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'elasticloadbalancing\': elasticloadbalancing, elasticloadbalancing; the request is rejected rather than served by an arbitrary one"}'`
(cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn, cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes).

The checks' measured AWS oracle for the calls that never got through: CreateTargetGroup replayed with identical settings returns the existing target group's ARN with HTTP 200; DescribeTargetGroups with a well-formed absent ARN returns wire code `TargetGroupNotFound` (400, Type=Sender); CreateTargetGroup reusing a live name with a different Port returns wire code `DuplicateTargetGroupName`.

## Steps to Reproduce
1. Point a botocore ELBv2 client (`boto3.client('elbv2')`) at the emulator.
2. Call `create_target_group(Name='tg-1', Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='instance')`.
   Expect: HTTP 200, CreateTargetGroupOutput with TargetGroups[0].TargetGroupArn.
   Observe: a JSON body `{"__type":"AmbiguousService",...}` that botocore cannot parse (ResponseParserError, invalid XML).
3. Call `describe_target_groups(TargetGroupArns=[<well-formed absent arn>])`: expect 400 `TargetGroupNotFound` in an XML ErrorResponse; observe the same unparseable JSON AmbiguousService body.
4. Repeat with a classic `boto3.client('elb')` call to confirm both claimants of the `elasticloadbalancing` signing name are affected.
