Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.batch-delete-partial-failure medialive.channel-restart-pipelines-recovering medialive.channel-schedule-actions-lifecycle medialive.channel-start-stop-lifecycle medialive.delete-input-attached-to-channel-rejected medialive.delete-running-channel-rejected medialive.signal-map-create-completes
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.batch-delete-partial-failure|_iam_role_for_medialive#3|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.channel-restart-pipelines-recovering|_iam_role_for_medialive#2|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.channel-schedule-actions-lifecycle|_iam_role_for_medialive#3|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.channel-start-stop-lifecycle|_iam_role_for_medialive#2|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.delete-input-attached-to-channel-rejected|_iam_role_for_medialive#3|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.delete-running-channel-rejected|_iam_role_for_medialive#2|ResponseParserError|role = iam.create_role(
Issue-Key: medialive.signal-map-create-completes|_iam_role_for_medialive#2|ResponseParserError|role = iam.create_role(
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# MediaLive channel/schedule/signal-map checks never reach MediaLive: the router refuses every sigv4 'iam' request as AmbiguousService in a non-AWS JSON body

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Cause 1 of 1: the emulator's request router resolves an incoming request to an emulated service by its sigv4 signing name, and two registered services claim the signing name 'iam'. Instead of serving the request it answers a refusal document, so every MediaLive behavioral check whose setup provisions the channel execution role through IAM dies before it issues a single MediaLive call.

AWS: a request signed with signing name 'iam' is served by IAM. iam.CreateRole returns HTTP 200 with a CreateRoleResult carrying Role.Arn, and an IAM failure is an AWS Query/XML <ErrorResponse> document with a modeled error code. No AWS model declares an 'AmbiguousService' error, and no AWS service answers an IAM request with a JSON body. Smithy evidence for the collision: services/service-com-amazonaws-iamtoolbox-authrequestservice declares sigv4Name 'iam' and arnNamespace 'iam' while its endpointPrefix is 'iam-toolbox', i.e. two distinct emulated services share one signing name but have distinct endpoint prefixes — which is exactly what AWS uses to tell them apart. MediaLive's own contract is unaffected and still the goal of these checks: CreateChannel requires a RoleArn the service can assume.

Emulator: the first IAM call of each check is answered with the body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'iam': iam, iam; the request is rejected rather than served by an arbitrary one"}. Two defects in one refusal: (a) the router cannot disambiguate the two claimants and serves neither, so every sigv4-'iam' operation fails; (b) the refusal is serialized as JSON to a client expecting IAM's Query/XML error document, so botocore raises ResponseParserError ('not well-formed (invalid token): line 1, column 0, invalid XML received') and no error code reaches the caller at all.

Why: any client workload that provisions an execution role before driving MediaLive gets an unparseable response rather than an IAM result or an IAM error. SDK error branching, retries and waiters are all defeated because no AWS error code is surfaced. In this suite it masks the entire MediaLive channel lifecycle, schedule, batch-delete and signal-map-discovery surface: 7 checks fail without exercising MediaLive at all, so MediaLive's own fidelity in those areas is currently unmeasurable.

Fix: the shared request router / service-registry claim resolution (disambiguate claimants by endpointPrefix and the request's host/URI rather than by sigv4 signing name alone), and the refusal serializer (emit the target service's protocol — Query/XML for IAM — instead of a JSON __type body). Related open issues describe the same router defect reached from other services and do NOT carry these MediaLive replay keys: '20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice.md' and '20261021T234200Z-textract-async-and-authorization-checks-never-reach-textract.md'.

Traces: medialive.batch-delete-partial-failure, medialive.channel-restart-pipelines-recovering, medialive.channel-schedule-actions-lifecycle, medialive.channel-start-stop-lifecycle, medialive.delete-input-attached-to-channel-rejected, medialive.delete-running-channel-rejected, medialive.signal-map-create-completes — each fails at its _iam_role_for_medialive() setup step, before any MediaLive operation is attempted.

## Evidence
All 7 checks fail identically (verdict [error]):

File "parsers.py", line 600, in _do_error_parse -> botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'iam\': iam, iam; the request is rejected rather than served by an arbitrary one"}'

Expected: iam.CreateRole returns HTTP 200 with Role.Arn (or, on failure, an IAM Query/XML ErrorResponse with a modeled IAM error code), after which the check proceeds to medialive.CreateChannel.
Actual: a JSON body with __type 'AmbiguousService'; the request is never routed to any service and no AWS error code reaches the SDK.

Smithy: service-com-amazonaws-iamtoolbox-authrequestservice -> sigv4Name 'iam', arnNamespace 'iam', endpointPrefix 'iam-toolbox' (sdkId 'IAM Toolbox'), colliding with IAM's own 'iam' signing name.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call any IAM operation against the emulator endpoint, e.g. iam.CreateRole(RoleName='sim2real-medialive-role-<rand>', AssumeRolePolicyDocument=<trust policy allowing sts:AssumeRole for medialive.amazonaws.com>).
3. Observe the response body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'iam': iam, iam; ..."}; botocore raises ResponseParserError because it expects IAM Query/XML.
4. Replay medialive.channel-start-stop-lifecycle (or any of the 7 listed checks) to confirm the MediaLive surface is never reached.
