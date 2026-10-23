Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-readiness-cell-and-tags-not-found
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-readiness-cell-and-tags-not-found route53-recovery-readiness-readiness-create-operations-access-denied route53-recovery-readiness-list-catalog-operations
Issue-Shape-Id: com.amazonaws.route53recoveryreadiness#Route53RecoveryReadiness
Issue-Key: route53-recovery-readiness|TagResource|4xx->4xx|status
Issue-Key: route53-recovery-readiness|UntagResource|4xx->4xx|status
Issue-Key: route53-recovery-readiness|ListTagsForResources|4xx->4xx|status
Issue-Key: route53-recovery-readiness|CreateReadinessCheck|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-readiness|ListRules|4xx->4xx|$.message
Issue-Commit: e2d4bffd2fe68cddce7016612710a92983157233

# Error responses diverge: wrong status, errortype header and validation message

> _Note: this issue was identified against the code base at commit `e2d4bffd2fe68cddce7016612710a92983157233`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three distinct error-path causes in the route53-recovery-readiness emulator. Each must be closed before this issue is resolved.

=== Cause 1 of 3: tag operations on a malformed/unknown resource ARN return 404 instead of 400 ===
AWS: TagResource, UntagResource and ListTagsForResources all take `ResourceArn` as an httpLabel on `/tags/{ResourceArn}` and all model BOTH ValidationException (400) and ResourceNotFoundException (404) (Smithy: TagResource errors = InternalServerException 500, ResourceNotFoundException 404, ValidationException 400; same for UntagResource and ListTagsForResources; ValidationException doc: "The input fails to satisfy the constraints specified by an AWS service."). Recorded behavior: for the ARN used by this fixture AWS answers 400 on all three operations -- it validates the ARN before any lookup, so an ARN that is not a well-formed Recovery Readiness resource ARN is a validation failure, not a missing resource. The requirement is the 400 status and the ValidationException error code.
Emulator: answers 404 on all three (detail: "status: expected 400, got 404"), i.e. it skips ARN validation and goes straight to a not-found lookup.
Why: a client distinguishes "I built a bad ARN" from "the resource is gone" by this status. AWS SDK retry/error-handling policy treats ResourceNotFoundException and ValidationException differently, and callers that branch on 404 to create-then-tag will loop or swallow a genuine request defect instead of surfacing it. Code that asserts a 400 for a malformed ARN fails against the emulator.
Fix: the shared ARN-validation step in front of the tagging handlers (TagResource / UntagResource / ListTagsForResources) in the route53recoveryreadiness crate: validate the `ResourceArn` httpLabel and raise ValidationException (400) before resolving the resource; only a well-formed ARN that resolves to nothing may yield ResourceNotFoundException (404).
Traces: route53-recovery-readiness-cell-and-tags-not-found#task#4 (TagResource), #task#5 (UntagResource), #task#6 (ListTagsForResources).

=== Cause 2 of 3: AccessDenied error is advertised with the wrong x-amzn-errortype ===
AWS: CreateReadinessCheck models AccessDeniedException at httpError 403 (Smithy: CreateReadinessCheck errors include AccessDeniedException errorKind=client httpError=403; AccessDeniedException doc: "User does not have sufficient access to perform this action."). Recorded behavior: the 403 response carries `x-amzn-errortype: AccessDenied`. For a REST-JSON service that header IS the error code -- it is what the SDK matches to pick the modeled exception -- so it is a requirement, not an example.
Emulator: returns the right status (403) but `x-amzn-errortype: AccessDeniedException` (detail: "$headers.x-amzn-errortype: value_mismatch (expected 'AccessDenied', got 'AccessDeniedException')"). It appears to emit the Smithy shape name verbatim rather than the wire error code the service uses.
Why: the SDK resolves a modeled exception from this header. With the wrong code, a client's `catch AccessDeniedException` / error-code comparison does not match, and the failure degrades to an unmodeled/unknown 403 -- so permission errors become unhandleable and authorization-handling branches never fire. If the same shape-name-as-code mapping is used service-wide, every other modeled error on this service is suspect too.
Fix: the error serializer for the route53recoveryreadiness service -- the mapping from modeled error shape to the `x-amzn-errortype` header value -- so AccessDeniedException serializes as `AccessDenied`.
Traces: route53-recovery-readiness-readiness-create-operations-access-denied#task#4 (CreateReadinessCheck).

=== Cause 3 of 3: ListRules invents an enum constraint on resourceType ===
AWS: ListRules.ResourceType is modeled as a plain `__string` bound to the `resourceType` query string -- there is NO enum, no pattern and no value list in the model (Smithy: ListRules.input.ResourceType target `__string`, httpQuery `resourceType`). Recorded behavior: an unsupported resource type is rejected with 400 and the fixed message `Unknown resource type.` -- a constant, not a constraint report built from the request.
Emulator: the status (400) and error code match, but it answers with a framework-generated enum-constraint report: `1 validation error detected: Value 'Not::A::RealType' at 'resourceType' failed to satisfy constraint: Member must be one of the supported resource types: AWS::ApiGateway::Stage, AWS::ApiGatewayV2::Stage, ... AWS::SQS::Queue`. That prose is evidence of the underlying defect: the emulator is applying a modeled-enum validator to a member the model does not constrain, and so publishes its own supported-type list as part of the wire contract.
Why: the emulator's list is the emulator's catalog, which Cause 1 of the companion ListRules issue shows is not the service's. A client (or a test) that reads the supported-type enumeration out of this error learns a type set AWS does not have, and the generated-constraint framing makes the rejection look like generic input validation rather than the service's own unknown-type rejection. The requirement is that ListRules reject an unsupported resourceType as the service does -- a 400 ValidationException from the operation's own check -- not that it emit a generated enum-constraint report; the recorded `Unknown resource type.` text is the example of that.
Fix: ListRules' input validation in the route53recoveryreadiness crate: drop the synthesized enum constraint on `ResourceType` and reject an unsupported value from the operation's handler instead.
Traces: route53-recovery-readiness-list-catalog-operations#task#6 (ListRules).

No other open issue exists for this service (list_existing_issues query 'route53-recovery-readiness' returned none), so nothing here is already covered.

## Evidence
Cause 1 -- expected vs actual (3 clusters, verdict=mismatch):
  fixture route53-recovery-readiness-cell-and-tags-not-found step task#4, TagResource (POST /tags/{ResourceArn}): expected status 400, got 404.
  same fixture step task#5, UntagResource (DELETE /tags/{ResourceArn}): expected status 400, got 404.
  same fixture step task#6, ListTagsForResources (GET /tags/{ResourceArn}): expected status 400, got 404.
  divergence_keys: service=route53-recovery-readiness|TagResource|status=400->404|keys=status, ...|UntagResource|status=400->404, ...|ListTagsForResources|status=400->404.

Cause 2 -- expected vs actual:
  fixture route53-recovery-readiness-readiness-create-operations-access-denied step task#4, CreateReadinessCheck: status 403 matches; header `$headers.x-amzn-errortype` expected 'AccessDenied', got 'AccessDeniedException'.
  divergence_key: service=route53-recovery-readiness|CreateReadinessCheck|status=403->403|keys=$headers.x-amzn-errortype.

Cause 3 -- expected vs actual:
  fixture route53-recovery-readiness-list-catalog-operations step task#6, ListRules with resourceType='Not::A::RealType': status 400 matches; `$.message` expected 'Unknown resource type.', got "1 validation error detected: Value 'Not::A::RealType' at 'resourceType' failed to satisfy constraint: Member must be one of the supported resource types: AWS::ApiGateway::Stage, AWS::ApiGatewayV2::Stage, AWS::AutoScaling::AutoScalingGroup, AWS::CloudWatch::Alarm, AWS::DynamoDB::Table, AWS::EC2::CustomerGateway, AWS::EC2::VPC, AWS::EC2::VPNConnection, AWS::EC2::VPNGateway, AWS::EC2::Volume, AWS::ElasticLoadBalancing::LoadBalancer, AWS::ElasticLoadBalancingV2::LoadBalancer, AWS::Lambda::Function, AWS::MSK::Cluster, AWS::RDS::DBCluster, AWS::Route53::HealthCheck, AWS::Route53RecoveryReadiness::DNSTargetResource, AWS::SNS::Subscription, AWS::SNS::Topic, AWS::SQS::Queue".
  divergence_key: service=route53-recovery-readiness|ListRules|status=400->400|keys=$.message.

Model consulted for all three causes: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/smithy-model/com-amazonaws-route53recoveryreadiness-route53recoveryreadiness.json (operations TagResource, UntagResource, ListTagsForResources, CreateReadinessCheck, ListRules; shapes ValidationException, ResourceNotFoundException, AccessDeniedException). Replay summary: emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so none of these ran against stale state.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it, then replay_wire_corpus().
2. Cause 1: replay fixture `route53-recovery-readiness-cell-and-tags-not-found`. Step task#4 tags the fixture's resource ARN (POST /tags/{ResourceArn}), task#5 untags it (DELETE), task#6 lists its tags (GET). Each returns 404 where the recording has 400. Re-replay all three steps to confirm a fix.
3. Cause 2: replay fixture `route53-recovery-readiness-readiness-create-operations-access-denied`, step task#4 (CreateReadinessCheck under a denied principal). Inspect the 403 response's `x-amzn-errortype` header: it reads `AccessDeniedException` instead of `AccessDeniedType`-free `AccessDenied`.
4. Cause 3: replay fixture `route53-recovery-readiness-list-catalog-operations`, step task#6 -- `GET /rules?resourceType=Not::A::RealType`. The 400 body's `message` is a generated enum-constraint report listing the emulator's own supported types instead of the service's unknown-type rejection.
5. A fix must re-replay all four fixture#step bindings named in trace_ids plus the representative route53-recovery-readiness-cell-and-tags-not-found#task#4.
