Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.custom-resource-registration-floor-patches-the-endpoint application-autoscaling.custom-resource-scheduled-action-patches-the-endpoint-when-it-fires application-autoscaling.custom-resource-stage-the-api-never-deployed-rejected application-autoscaling.custom-resource-step-policy-alarm-patches-the-endpoint-up-to-the-ceiling application-autoscaling.custom-resource-target-tracking-with-scale-in-disabled-still-scales-out
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling|PutIntegration|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# custom-resource Application Auto Scaling checks are blocked: API Gateway PutIntegration answers NotImplemented

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Cause 1 of 1: the custom-resource scaling scenarios cannot be exercised at all because the API Gateway operation they build their custom-resource endpoint with is an unimplemented scaffold stub, and the replay report does NOT mark these checks as already filed.
AWS: PutIntegration is a modeled API Gateway operation that returns the created integration (HTTP 2xx). A custom-resource scalable target (ServiceNamespace 'custom-resource') needs that REST endpoint to exist so Application Auto Scaling can GET/PATCH the registered resource's capacity.
Emulator: every one of the five checks dies at the setup call with HTTP error code NotImplemented and message 'operation PutIntegration is scaffolded but not implemented in this emulator', so no custom-resource registration, scheduled action, step policy or target-tracking behavior is reachable.
Why: the whole custom-resource namespace of Application Auto Scaling is untestable and any client that scales a custom resource through an API Gateway endpoint cannot run against the emulator.
Fix: implement the API Gateway PutIntegration handler (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_integration.rs); then re-replay the five checks to see the Application Auto Scaling custom-resource behavior underneath.
Traces: the five application-auto-scaling.custom-resource-* checks listed below.

## Evidence
Replay report, all five checks error identically at setup:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutIntegration operation: operation PutIntegration is scaffolded but not implemented in this emulator
Checks: application-auto-scaling.custom-resource-registration-floor-patches-the-endpoint, .custom-resource-scheduled-action-patches-the-endpoint-when-it-fires, .custom-resource-stage-the-api-never-deployed-rejected, .custom-resource-step-policy-alarm-patches-the-endpoint-up-to-the-ceiling, .custom-resource-target-tracking-with-scale-in-disabled-still-scales-out.
Unlike DeleteScheduledAction / GetFunctionConfiguration / CreateVpc, the report does not mark these as ALREADY FILED, and no open issue names PutIntegration (checked via the issue listing).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. With botocore: apigateway.create_rest_api(...), create_resource(...), put_method(...), then put_integration(restApiId=..., resourceId=..., httpMethod='GET', type='MOCK').
3. Observe the NotImplemented client error instead of the created integration; every custom-resource Application Auto Scaling check aborts here.
