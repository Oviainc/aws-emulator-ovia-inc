Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-quicksetup-not-found-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-quicksetup-not-found-error-paths
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|status
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|status
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|status
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|status
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# ssm-quicksetup tagging and configuration-definition routes never emit the front-door 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfigurationDefinition

## Description
Four more ssm-quicksetup routes that take a resource ARN as an HTTP label answer with a handler-level rejection (ResourceNotFoundException / ValidationException) where the recorded service refuses the request at the front door with 401 UnauthorizedException. The open issue 20261022T034519Z-ssm-quicksetup-error-paths-wrong-rejection-class-unvalidated.md covers the same fault on Get/Update/DeleteConfigurationManager only; these four operations are not in it. 4 causes; all must be closed.

Cause 1 of 4: UpdateConfigurationDefinition on a manager ARN that names no manager.
AWS: recorded behavior only - the model binds ResourceNotFoundException (httpError 404) and ValidationException (httpError 400) to PUT /configurationDefinition/{ManagerArn}/{Id} and does not model UnauthorizedException at all, so the recorded 401 comes from the REST front door refusing authorization of the target ARN before input validation and before any resource lookup. The recording shows status 401, header x-amzn-errortype: UnauthorizedException, and the fixed front-door body member $.message = 'Unauthorized' (lowercase member, unlike the service's own modeled ValidationException.Message / ResourceNotFoundException.Message). The body is wholly fixed text - nothing is interpolated from the request.
Emulator: status 400 with header x-amzn-errortype: ValidationException and no $.message member at all - it pattern-validates the ARN/Id labels (Smithy: UpdateConfigurationDefinitionInput.ManagerArn @httpLabel @required pattern ^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$, Id @httpLabel @required pattern ^[a-z0-9-]{1,20}$) instead of refusing at the front door.
Why: 401 is an authorization outcome that drives credential/permission re-acquisition and is not retryable with the same input; 400 tells the client its request was malformed and 404 tells it to create the resource. A client that loses access to a definition is told its ARN is badly formed, so it never surfaces the authorization failure.
Fix: the request-authorization stage that fronts the ARN-labelled routes in the ssm-quicksetup service crate - it must run before UpdateConfigurationDefinition's input validation and emit 401 UnauthorizedException with body {"message": "Unauthorized"}.
Traces: ssm-quicksetup-not-found-error-paths#task#4

Cause 2 of 4: TagResource on a resource ARN that names no resource.
AWS: recorded behavior only - PUT /tags/{ResourceArn} with an ARN naming no resource answers status 401, x-amzn-errortype: UnauthorizedException, $.message = 'Unauthorized'. The model binds ResourceNotFoundException (404) and ValidationException (400) to TagResource but not UnauthorizedException, confirming the 401 is produced ahead of the handler.
Emulator: status 404 with x-amzn-errortype: ResourceNotFoundException and no $.message member.
Why: on a write path a 404 reads as 'resource absent' and sends a provisioning client into create-then-retag logic against a resource it is not entitled to touch, instead of failing the call as unauthorized.
Fix: same front-door authorization stage, applied to the /tags/{ResourceArn} PUT route, before the handler's resource lookup.
Traces: ssm-quicksetup-not-found-error-paths#task#7

Cause 3 of 4: ListTagsForResource on a resource ARN that names no resource.
AWS: recorded behavior only - GET /tags/{ResourceArn} answers status 401, x-amzn-errortype: UnauthorizedException, $.message = 'Unauthorized'.
Emulator: status 404 with x-amzn-errortype: ResourceNotFoundException and no $.message member.
Why: a read that reports 404 lets a tag-reconciliation pass treat the resource as gone and prune its tag state, where AWS would have told it the read was not authorized.
Fix: same front-door authorization stage, applied to the /tags/{ResourceArn} GET route.
Traces: ssm-quicksetup-not-found-error-paths#task#8

Cause 4 of 4: UntagResource on a resource ARN that names no resource.
AWS: recorded behavior only - DELETE /tags/{ResourceArn}?tagKeys=... answers status 401, x-amzn-errortype: UnauthorizedException, $.message = 'Unauthorized'.
Emulator: status 404 with x-amzn-errortype: ResourceNotFoundException and no $.message member.
Why: a delete answered 404 reads as 'already removed', so a cleanup routine declares success on tags it never had permission to remove.
Fix: same front-door authorization stage, applied to the /tags/{ResourceArn} DELETE route.
Traces: ssm-quicksetup-not-found-error-paths#task#9

## Evidence
Cause 1 - ssm-quicksetup-not-found-error-paths#task#4, UpdateConfigurationDefinition: expected status 401, $.message 'Unauthorized', x-amzn-errortype 'UnauthorizedException'; actual status 400, $.message missing, x-amzn-errortype 'ValidationException'.
Cause 2 - ssm-quicksetup-not-found-error-paths#task#7, TagResource: expected 401 / $.message 'Unauthorized' / 'UnauthorizedException'; actual 404 / $.message missing / 'ResourceNotFoundException'.
Cause 3 - ssm-quicksetup-not-found-error-paths#task#8, ListTagsForResource: expected 401 / $.message 'Unauthorized' / 'UnauthorizedException'; actual 404 / $.message missing / 'ResourceNotFoundException'.
Cause 4 - ssm-quicksetup-not-found-error-paths#task#9, UntagResource: expected 401 / $.message 'Unauthorized' / 'UnauthorizedException'; actual 404 / $.message missing / 'ResourceNotFoundException'.
In all four the error message is the fixed string 'Unauthorized' carried in the lowercase member $.message; no part of it is request- or environment-derived. The ARN values in the fixtures are examples of 'an ARN this caller cannot act on', not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: PUT /configurationDefinition/{ManagerArn}/{Id} with a manager ARN that names no manager (fixture ssm-quicksetup-not-found-error-paths, step task#4). The emulator answers 400 ValidationException instead of 401 UnauthorizedException with body {"message": "Unauthorized"}.
3. Cause 2: PUT /tags/{ResourceArn} with Tags set, for an ARN naming no resource (same fixture, step task#7). Answers 404 ResourceNotFoundException instead of 401.
4. Cause 3: GET /tags/{ResourceArn} for the same ARN (same fixture, step task#8). Answers 404 ResourceNotFoundException instead of 401.
5. Cause 4: DELETE /tags/{ResourceArn}?tagKeys=<key> for the same ARN (same fixture, step task#9). Answers 404 ResourceNotFoundException instead of 401.
