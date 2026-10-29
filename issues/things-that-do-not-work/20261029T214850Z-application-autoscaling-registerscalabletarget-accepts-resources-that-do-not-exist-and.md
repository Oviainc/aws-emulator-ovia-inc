Issue-Source: trace-sweep-divergence
Issue-Trace: application-autoscaling-register-absent-resource-per-namespace-errors
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-autoscaling-register-absent-resource-per-namespace-errors application-autoscaling-malformed-resource-id-and-dimension-mismatch
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling|RegisterScalableTarget|4xx->2xx|$.Message
Issue-Key: application-autoscaling|RegisterScalableTarget|4xx->2xx|$.__type
Issue-Key: application-autoscaling|RegisterScalableTarget|4xx->2xx|status
Issue-Key: application-autoscaling|DescribeScalableTargets|2xx->2xx|$.ScalableTargets[*]
Issue-Key: application-autoscaling|RegisterScalableTarget|4xx->4xx|$.Message
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# RegisterScalableTarget accepts resources that do not exist and misreports dimension mismatches (3 causes)

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Three root causes in RegisterScalableTarget's validation path: it never checks that the resource it is asked to scale exists in the owning service, so registration falsely succeeds and the phantom target is then served back by DescribeScalableTargets; and when a ResourceId does not match its ScalableDimension it refuses with its own prose instead of AWS's fixed text. Open issue 20261024T232810Z-application-autoscaling-registerscalabletarget-in-the-lambda-namespace-reports-a-service.md covers a different RegisterScalableTarget defect (the reported service-linked RoleARN is never minted in IAM) and carries none of the fixture/step keys below.

Cause 1 of 3: registration of a resource that does not exist in the owning service returns 200 instead of refusing.
AWS: Smithy RegisterScalableTarget models ValidationException (@httpError 400, awsQueryError code ValidationException) and its ScalableDimension/ServiceNamespace members are enums spanning lambda, dynamodb, ecs, sagemaker, cassandra, kafka, elasticache, neptune, comprehend, appstream, rds, ec2, elasticmapreduce, custom-resource and workspaces. Registration is not a bare bookkeeping write: AWS resolves the ResourceId against the owning service and refuses when the underlying resource is absent, with HTTP 400 ValidationException whose message follows the template 'Validation failed for resource: <ResourceId>, scalable dimension: <ScalableDimension>. Reason: <per-namespace reason>'. Fixed text: 'Validation failed for resource: ', ', scalable dimension: ', '. Reason: '. Variable parts: the request's ResourceId and ScalableDimension, plus a reason clause determined by the namespace and the resource that could not be resolved — for the lambda namespace it names the alias ARN that was constructed from the request and not found ('Cannot find alias arn: <arn of the function:alias named by ResourceId>'); the recording exercises one absent resource per supported namespace, each with its own reason clause for the resource kind that namespace scales.
Emulator: HTTP 200 with a ScalableTargetARN and no error body at all — $.__type and $.Message are absent where the fixture expects 'ValidationException' and the validation message. This happens for every namespace in the fixture (7 registrations).
Why: the single most important thing this call tells a client is whether the thing it wants to scale can be scaled. A 200 means provisioning code, CloudFormation-style convergence and test suites all record success for a target that can never scale anything, and the failure surfaces much later (or never) as capacity that silently does not move.
Fix: RegisterScalableTarget's validation in this service's ops module (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/register_scalable_target.rs) — resolve the ResourceId against the owning service's emulated state per namespace before writing the target, and raise the modeled ValidationException with the 'Validation failed for resource: ... Reason: ...' template when it does not resolve.

Cause 2 of 3: DescribeScalableTargets then reports the phantom target.
AWS: Smithy DescribeScalableTargets returns ScalableTargets, the list of targets actually registered for the queried namespace. Because the registration in cause 1 is refused, the recorded response for the follow-up describe is an EMPTY ScalableTargets list.
Emulator: returns one fully-populated ScalableTarget element per phantom registration — ServiceNamespace, ResourceId, ScalableDimension, MinCapacity, MaxCapacity, a service-linked RoleARN, CreationTime, SuspendedState and ScalableTargetARN — where the fixture expects no element.
Why: the describe is what clients and reconciliation loops read back to confirm state, so the phantom is not merely an un-refused write: it is published as real, and anything that enumerates scalable targets (dashboards, cleanup jobs, drift detection) acts on a target whose resource does not exist.
Fix: this follows from cause 1's missing pre-flight (no target is stored, so nothing is listed), but the fixer must re-replay this step too rather than assume it: DescribeScalableTargets' handler and this service's target store must agree that an un-registered target is not listed.
Traces: application-autoscaling-register-absent-resource-per-namespace-errors#task#6

Cause 3 of 3: a ResourceId that does not match its ScalableDimension is refused with emulator-authored prose.
AWS: the recorded refusal is HTTP 400 ValidationException (both already match) with the Message 'Unsupported service namespace, resource type or scalable dimension' — a single fixed sentence with NO request-derived parts: AWS deliberately does not echo the resource id, the expected resource type or the dimension here.
Emulator: substitutes a constructed explanation, 'ResourceId not valid for ScalableDimension <dim>: <resourceId> must name a resource of type <type> in the <ns> service namespace, of the form <shape>', interpolating request values and a form hint AWS never emits.
Why: the message is part of the API contract for this refusal class; clients, CLI users and conformance suites that match AWS's fixed sentence do not match, and the emulator's extra detail implies a diagnosis (the dimension is fine, only the id's shape is wrong) that AWS does not make.
Fix: the shared RegisterScalableTarget refusal-message builder — emit the fixed 'Unsupported service namespace, resource type or scalable dimension' text for the namespace/resource-type/dimension mismatch class instead of a per-request explanation.
Traces: application-autoscaling-malformed-resource-id-and-dimension-mismatch#task#4 (and task#5, task#6, task#7, task#8 in the same cluster)

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 130 steps, 103 passed, 27 mismatched, score 79.2%. Three of the nine divergence clusters are these causes; none carries a stub_path or an owned_by.

Cause 1 — fixture application-autoscaling-register-absent-resource-per-namespace-errors, step task#5, RegisterScalableTarget, expected 400 got 200: 'status: expected 400, got 200; $.__type: missing (expected ValidationException, got None); $.Message: missing (expected "Validation failed for resource: function:<fn>:<alias>, scalable dimension: lambda:function:ProvisionedConcurrency. Reason: Cannot find alias arn: arn:aws:lambda:<region>:<account>:function:<fn>:<alias>", got None)'. Cluster covers 7 steps — task#5, 7, 9, 11, 13, 15, 17 — one per namespace exercised.

Cause 2 — same fixture, step task#6, DescribeScalableTargets, 200 -> 200: '$.ScalableTargets[0]: extra (expected None, got {ServiceNamespace: lambda, ResourceId: function:<fn>:<alias>, ScalableDimension: lambda:function:ProvisionedConcurrency, MinCapacity: 1, MaxCapacity: 2, RoleARN: <service-linked role arn>, CreationTime: <ts>, SuspendedState: {...}, ScalableTargetARN: <arn>})'. Cluster covers 7 steps — task#6, 8, 10, 12, 14, 16, 18 — the describe that follows each phantom registration.

Cause 3 — fixture application-autoscaling-malformed-resource-id-and-dimension-mismatch, step task#4, RegisterScalableTarget, 400 -> 400: '$.Message: expected "Unsupported service namespace, resource type or scalable dimension", got "ResourceId not valid for ScalableDimension ecs:service:DesiredCount: service/<name> must name a resource of type service in the ecs service namespace, of the form service/{cluster name}/{service name}"'. Cluster covers 5 steps — task#4 through task#8 — i.e. the same substituted message for several namespace/dimension mismatches.

Model grounding (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json): RegisterScalableTarget errors = ConcurrentUpdateException(500), InternalServiceException(500), LimitExceededException(400), ValidationException(400); output carries only ScalableTargetARN. DescribeScalableTargets output = ScalableTargets + NextToken. ServiceNamespace and ScalableDimension are closed enums (15 and 24 values).

## Steps to Reproduce
Boot the emulator and replay the two fixtures named above, or drive them directly:

Causes 1-2: without creating the underlying resource, call register_scalable_target(ServiceNamespace='lambda', ResourceId='function:<absent function>:<alias>', ScalableDimension='lambda:function:ProvisionedConcurrency', MinCapacity=1, MaxCapacity=2) — expect HTTP 400 ValidationException 'Validation failed for resource: <ResourceId>, scalable dimension: lambda:function:ProvisionedConcurrency. Reason: Cannot find alias arn: <constructed alias arn>'; observe HTTP 200 with a ScalableTargetARN. Then call describe_scalable_targets(ServiceNamespace='lambda') — expect an empty ScalableTargets list; observe the phantom target listed. Repeat per namespace (dynamodb table, ecs service, sagemaker variant, cassandra table, elasticache replication group, neptune cluster, ...) against a resource that was never created: each must be refused with that namespace's reason clause.

Cause 3: call register_scalable_target with a ResourceId that does not match its ScalableDimension — e.g. ServiceNamespace='ecs', ResourceId='service/<name>' (missing the cluster segment) with ScalableDimension='ecs:service:DesiredCount' — and compare only the Message (status 400 and ValidationException already match): AWS answers the fixed sentence 'Unsupported service namespace, resource type or scalable dimension'; the emulator answers 'ResourceId not valid for ScalableDimension ...: ... must name a resource of type ... of the form ...'.
