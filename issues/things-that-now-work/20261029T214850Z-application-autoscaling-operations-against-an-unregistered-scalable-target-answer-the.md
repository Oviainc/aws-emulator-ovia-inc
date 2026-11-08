Issue-Source: trace-sweep-divergence
Issue-Trace: application-autoscaling-error-mutations-without-registered-target
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-autoscaling-error-mutations-without-registered-target applicationautoscaling-registration-error-envelopes application-autoscaling-error-tag-resource-not-found application-autoscaling-malformed-resource-id-and-dimension-mismatch
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling|DeleteScheduledAction|4xx->4xx|$.Message
Issue-Key: application-autoscaling|GetPredictiveScalingForecast|4xx->4xx|$.Message
Issue-Key: application-autoscaling|GetPredictiveScalingForecast|4xx->4xx|$.__type
Issue-Key: application-autoscaling|ListTagsForResource|4xx->4xx|status
Issue-Key: application-autoscaling|TagResource|4xx->4xx|$.Message
Issue-Key: application-autoscaling|TagResource|4xx->4xx|$.__type
Issue-Key: application-autoscaling|UntagResource|4xx->4xx|$.Message
Issue-Key: application-autoscaling|UntagResource|4xx->4xx|$.__type
Issue-Key: application-autoscaling|PutScalingPolicy|4xx->4xx|$.Message
Issue-Key: application-autoscaling|PutScalingPolicy|4xx->4xx|$.__type
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# Operations against an unregistered scalable target answer the wrong error type, status and message (6 causes)

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledAction

## Description
Six independent root causes, all in this service's refusal path for a request that addresses a scalable target (or a scalable-target ARN) that is not registered. In every case the emulator answers, but it answers with the wrong exception identity, the wrong HTTP status, or a message of its own authorship instead of AWS's. Related open issue 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md describes the tag-ARN and 'registered'-wording families from the BEHAVIORAL corpus; it does not carry any of the wire fixture/step keys below (the replay reports no owner for them), and its cause 3 asserts the opposite status (404) from what these recordings show, so these are filed here and must be closed against these fixtures.

Cause 1 of 6: DeleteScheduledAction reports the missing scheduled action instead of the missing scalable target.
AWS: Smithy DeleteScheduledAction models ObjectNotFoundException (@httpError 400, awsQueryError code ObjectNotFoundException), documented as thrown 'for any operation that depends on the existence of a scalable target ... if the scalable target with the specified service namespace, resource ID, and scalable dimension does not exist'. The recorded response for a delete aimed at a namespace/resource/dimension triple with NO registered target is HTTP 400 with Message 'No scalable target registered for service namespace: <ServiceNamespace>, resource ID: <ResourceId>, scalable dimension: <ScalableDimension>'. Fixed text: 'No scalable target registered for service namespace: ', ', resource ID: ', ', scalable dimension: '; variable parts: the three request members. The target-existence check PRECEDES the scheduled-action lookup, so the scheduled action's name never appears.
Emulator: HTTP 400 with the scheduled-action template instead — 'No scheduled action found for service namespace: <ns>, resource ID: <rid>, scalable dimension: <dim>, scheduled action name: <ScheduledActionName>' — i.e. it skips the target check and reports the inner lookup.
Why: a client cannot tell 'this target was never registered' from 'this target exists but has no such action', so cleanup and idempotency logic that branches on the two keeps deleting against a triple that will never accept anything.
Fix: the DeleteScheduledAction handler in this service's ops module (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/) — resolve the scalable target first and raise the scalable-target ObjectNotFoundException template before looking the action up.
Traces: application-autoscaling-error-mutations-without-registered-target#task#4

Cause 2 of 6: GetPredictiveScalingForecast answers a ValidationException about a missing policy where AWS refuses the operation outright.
AWS: recorded behavior only — the Smithy model declares only InternalServiceException and ValidationException for GetPredictiveScalingForecast, and AccessDeniedException is not in its modeled error list. The recording shows HTTP 400 with __type 'AccessDeniedException' and Message 'GetPredictiveScalingForecast is not supported.' — an entirely fixed message with no request-derived parts. The refusal does not depend on whether a predictive policy or a target exists.
Emulator: HTTP 400 with __type 'ValidationException' and Message 'No predictive scaling policy found for service namespace: <ns>, resource ID: <rid>, scalable dimension: <dim>, policy name: <PolicyName>'.
Why: the SDK raises the wrong exception class (ValidationException instead of AccessDeniedException), so a client that treats 'operation not supported in this namespace' as a permanent, non-retryable capability answer instead reads a per-request validation problem and retries with different arguments forever.
Fix: the GetPredictiveScalingForecast handler — answer the fixed AccessDeniedException refusal for namespaces where the operation is not offered, ahead of any policy lookup.
Traces: application-autoscaling-error-mutations-without-registered-target#task#6

Cause 3 of 6: ListTagsForResource on a scalable-target ARN that resolves to nothing uses HTTP 404 where the wire answers 400.
AWS: Smithy ListTagsForResource models only ResourceNotFoundException and annotates it @httpError 404, but the RECORDED wire response for a well-formed ARN (matching the modeled pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$) naming no registered target is HTTP 400; the body matched the fixture exactly, so only the status line diverges. The recording is the wire contract here, and the modeled 404 is what the emulator is following.
Emulator: HTTP 404 with an otherwise byte-correct error body.
Why: clients and proxies branch on the status class before parsing the body — a 404 is routed as 'no such endpoint/route' and is retried or surfaced differently from a 400 client-error, and SDK retry classification differs between the two.
Fix: the ListTagsForResource handler's error channel — emit the not-found refusal with HTTP status 400 rather than the modeled 404.
Traces: application-autoscaling-error-tag-resource-not-found#task#1

Cause 4 of 6: TagResource on an absent scalable-target ARN is answered as a permission failure.
AWS: the recording shows HTTP 400 with __type 'ResourceNotFoundException' and Message 'Scalable Target not found' — fixed text in full, with no request-derived or state-derived parts (the ARN is carried in the error's ResourceName member, not interpolated into the message). Smithy TagResource models ResourceNotFoundException alongside TooManyTagsException and ValidationException, so the absent-target case is the ResourceNotFound one and not a validation failure.
Emulator: HTTP 400 with __type 'ValidationException' and Message 'Access Denied'. The handler does this on purpose — services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/tag_resource.rs returns validation(ACCESS_DENIED) when state.arn_index does not resolve the ARN, on the stated theory that a write to an unowned target is reported as a permission failure. The recording contradicts that.
Why: 'Access Denied' is indistinguishable from a real authorization failure, so a client retries with different credentials or escalates to an operator instead of recreating/re-registering the target; nothing keyed on ResourceNotFoundException ever fires.
Fix: tag_resource.rs — raise ResourceNotFoundException ('Scalable Target not found', ResourceName = the request's ResourceARN) at HTTP 400 when the ARN resolves to no registered target.
Traces: application-autoscaling-error-tag-resource-not-found#task#2

Cause 5 of 6: UntagResource on an absent scalable-target ARN has the same defect.
AWS: HTTP 400, __type 'ResourceNotFoundException', Message 'Scalable Target not found' (fixed text in full). Smithy UntagResource models ResourceNotFoundException and ValidationException.
Emulator: HTTP 400, __type 'ValidationException', Message 'Access Denied'.
Why: same as cause 4 — a tag-removal loop over stale ARNs reads an authorization problem rather than an absent resource and cannot make progress.
Fix: the UntagResource handler, through the same shared ARN-resolution refusal as cause 4, so all three tag operations agree.
Traces: application-autoscaling-error-tag-resource-not-found#task#3

Cause 6 of 6: PutScalingPolicy against a namespace/dimension pair with no registered target reports a member-constraint violation instead of the missing target.
AWS: Smithy PutScalingPolicy models ObjectNotFoundException (@httpError 400). The recorded response when the triple names no registered target — including when the ScalableDimension does not belong to the given ServiceNamespace — is HTTP 400, __type 'ObjectNotFoundException', Message 'No scalable target registered for service namespace: <ServiceNamespace>, resource ID: <ResourceId>, scalable dimension: <ScalableDimension>'; the fixed text is the three labelled clauses and the variable parts are the request's own members. AWS does not treat a cross-namespace dimension as a constraint violation on the member at this point: nothing is registered, so the not-found answer is the one it gives.
Emulator: HTTP 400, __type 'ValidationException', Message "Value '<dim>' at 'scalableDimension' failed to satisfy constraint: Member must be a scalable dimension of the '<ns>' service namespace".
Why: both the exception class and the diagnosis are wrong — a client is told its request is malformed when the actual state is that no target is registered, so it rewrites the request instead of calling RegisterScalableTarget.
Fix: the PutScalingPolicy handler's pre-flight — look the target up by (ServiceNamespace, ResourceId, ScalableDimension) and raise ObjectNotFoundException with the registered-target template; reserve the member-constraint message for inputs the model itself rejects.
Traces: application-autoscaling-malformed-resource-id-and-dimension-mismatch#task#9

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 130 steps, 103 passed, 27 mismatched, score 79.2%; emulator_up true, bootstrap_signal false. Six of the nine divergence clusters are these causes (no stub_path, no owned_by on any of them).

Cause 1 — fixture application-autoscaling-error-mutations-without-registered-target, step task#4, DeleteScheduledAction, 400 -> 400: $.Message expected 'No scalable target registered for service namespace: custom-resource, resource ID: <resourceId>, scalable dimension: custom-resource:ResourceType:Property', got 'No scheduled action found for service namespace: custom-resource, resource ID: <resourceId>, scalable dimension: custom-resource:ResourceType:Property, scheduled action name: <actionName>'.

Cause 2 — same fixture, step task#6, GetPredictiveScalingForecast, 400 -> 400: $.__type expected 'AccessDeniedException', got 'ValidationException'; $.Message expected 'GetPredictiveScalingForecast is not supported.', got 'No predictive scaling policy found for service namespace: custom-resource, resource ID: <resourceId>, scalable dimension: custom-resource:ResourceType:Property, policy name: <policyName>'. Also affects applicationautoscaling-registration-error-envelopes#task#9 (same cluster).

Cause 3 — fixture application-autoscaling-error-tag-resource-not-found, step task#1, ListTagsForResource: status expected 400, got 404 (body matched). Also affects applicationautoscaling-registration-error-envelopes#task#8.

Cause 4 — same fixture, step task#2, TagResource, 400 -> 400: $.__type expected 'ResourceNotFoundException', got 'ValidationException'; $.Message expected 'Scalable Target not found', got 'Access Denied'.

Cause 5 — same fixture, step task#3, UntagResource, 400 -> 400: identical pair of mismatches ('ResourceNotFoundException'/'Scalable Target not found' vs 'ValidationException'/'Access Denied').

Cause 6 — fixture application-autoscaling-malformed-resource-id-and-dimension-mismatch, step task#9, PutScalingPolicy, 400 -> 400: $.__type expected 'ObjectNotFoundException', got 'ValidationException'; $.Message expected 'No scalable target registered for service namespace: dynamodb, resource ID: table/<table>, scalable dimension: ecs:service:DesiredCount', got "Value 'ecs:service:DesiredCount' at 'scalableDimension' failed to satisfy constraint: Member must be a scalable dimension of the 'dynamodb' service namespace".

Model grounding (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json): DeleteScheduledAction errors = ConcurrentUpdate/InternalService/ObjectNotFound(400)/Validation(400); PutScalingPolicy errors include ObjectNotFoundException(400); GetPredictiveScalingForecast errors = InternalService/Validation only (no AccessDeniedException); Tag/Untag/ListTagsForResource all model ResourceNotFoundException @httpError 404 while the wire recordings answer 400. Handler evidence for causes 4/5: src/ops/tag_resource.rs lines 192-203 return validation(ACCESS_DENIED) on an unresolved ARN.

## Steps to Reproduce
Boot the emulator and replay the three fixtures named above, or drive them directly:

Causes 1-2: register nothing, then call delete_scheduled_action(ServiceNamespace='custom-resource', ResourceId=<any unregistered resource id>, ScalableDimension='custom-resource:ResourceType:Property', ScheduledActionName=<any name>) — expect 400 ObjectNotFoundException 'No scalable target registered for service namespace: ..., resource ID: ..., scalable dimension: ...'; observe the 'No scheduled action found ...' text. Then call get_predictive_scaling_forecast on the same triple with any PolicyName/StartTime/EndTime — expect 400 AccessDeniedException 'GetPredictiveScalingForecast is not supported.'; observe 400 ValidationException about a missing predictive scaling policy.

Causes 3-5: build a well-formed scalable-target ARN that matches the modeled pattern but names no registered target, then call list_tags_for_resource (expect HTTP 400, observe 404), tag_resource with any tag map and untag_resource with any key list (expect HTTP 400 ResourceNotFoundException 'Scalable Target not found'; observe HTTP 400 ValidationException 'Access Denied').

Cause 6: with no target registered, call put_scaling_policy(ServiceNamespace='dynamodb', ResourceId='table/<table>', ScalableDimension='ecs:service:DesiredCount', PolicyName=<any>) — expect 400 ObjectNotFoundException with the 'No scalable target registered for ...' template; observe 400 ValidationException with the scalableDimension member-constraint sentence.
