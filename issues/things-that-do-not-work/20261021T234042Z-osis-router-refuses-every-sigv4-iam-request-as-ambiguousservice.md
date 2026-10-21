Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.validate-pipeline-missing-permission-rejected
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.validate-pipeline-missing-permission-rejected|run#6|ResponseParserError|role_arn = iam.create_role(
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# Router refuses every sigv4 'iam' request as AmbiguousService (iam vs iam-toolbox signing-name collision), in a non-AWS JSON body

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Cause 1 of 1: the emulator's request router resolves a request to an emulated service by its sigv4 signing name, and two registered services claim the signing name 'iam', so it refuses the request instead of serving it.

AWS: IAM requests signed with service name 'iam' are served by IAM; CreateRole returns 200 with the Role (CreateRoleResult), and IAM errors are AWS Query/XML <ErrorResponse> documents with a modeled error code. There is no 'AmbiguousService' error in any AWS model. Smithy evidence for the collision: services/service-com-amazonaws-iamtoolbox-authrequestservice declares sigv4Name "iam" and arnNamespace "iam" while its endpointPrefix is "iam-toolbox" — i.e. two distinct emulated services share the signing name but have distinct endpoint prefixes, which is what AWS uses to tell them apart.

Emulator: the IAM call is answered with the body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'iam': iam, iam; the request is rejected rather than served by an arbitrary one"}. Two problems in one refusal: (a) the router cannot disambiguate the two claimants and serves neither, so every sigv4-'iam' operation fails; (b) the refusal is serialized as JSON to a client that expects IAM's Query/XML error document, so botocore cannot even parse it — it raises ResponseParserError ("not well-formed (invalid token): line 1, column 0, invalid XML received") and no error code reaches the caller.

Why: a client doing any IAM work against this emulator gets an unparseable response rather than an IAM result or an IAM error; SDK error handling, retries and error-code branching are all defeated because no AWS error code is surfaced at all. Any check or workload whose setup touches IAM is blocked regardless of the service under test.

Fix: the shared request router / service-registry claim resolution (disambiguate claimants by endpointPrefix and the request's host/URI, not by sigv4 signing name alone), and the refusal serializer (emit the target service's protocol — Query/XML for IAM — rather than a JSON __type body).

Attribution note: this was found through an OSIS behavioral check, whose service token is 'osis' (bootstrapped), so the issue is filed under osis; the defective component is the shared router, not an OSIS handler. The collision also means a future IAM bootstrap cannot be exercised until the claim is resolved.

Traces: osis.validate-pipeline-missing-permission-rejected — the check's sts.GetCallerIdentity succeeds, and the very next call, iam.CreateRole, is refused as above.

## Evidence
osis.validate-pipeline-missing-permission-rejected [error]:
File "parsers.py", line 600, in _do_error_parse -> botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'iam\': iam, iam; the request is rejected rather than served by an arbitrary one"}'

Expected: iam.CreateRole returns 200 with Role.Arn (or, on failure, an IAM Query/XML ErrorResponse with a modeled error code).
Actual: a JSON body with __type "AmbiguousService" that no AWS SDK can parse for the IAM Query protocol; the request is never routed to a service.

Smithy: service-com-amazonaws-iamtoolbox-authrequestservice -> sigv4Name "iam", arnNamespace "iam", endpointPrefix "iam-toolbox" (sdkId "IAM Toolbox"), which collides with IAM's own 'iam' signing name.

## Steps to Reproduce
1. Boot the emulator.
2. Call sts.GetCallerIdentity — it succeeds, confirming the endpoint is serving.
3. Call any IAM operation against the same endpoint, e.g. iam.CreateRole(RoleName="sim2real-osis-noperm-role-<rand>", AssumeRolePolicyDocument=<trust policy allowing sts:AssumeRole for the account root>).
4. Observe the response body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'iam': iam, iam; ..."}; botocore raises ResponseParserError because it expects IAM Query/XML.
5. Replay osis.validate-pipeline-missing-permission-rejected to confirm.
