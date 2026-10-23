Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method
Issue-Key: apigateway__cloudwatch|UpdateStage|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__cloudwatch: the per-method-metrics check aborts because apigateway UpdateStage answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: UpdateStage answers 501 NotImplemented, so per-method metric settings can never be enabled and the check never reaches its CloudWatch assertions.
AWS: Smithy: UpdateStage takes a list of patch operations and returns the updated Stage (HTTP 200); its modeled errors are BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429 and UnauthorizedException 401. Enabling detailed metrics for one method is done by patching /~1<resource>/<METHOD>/metricsEnabled on the stage.
Emulator: HTTP 501 with error code `NotImplemented` and message "operation UpdateStage is scaffolded but not implemented in this emulator".
Why: `NotImplemented` is not in UpdateStage's modeled error set, so no client retry or error branch matches it; and because the stage's methodSettings can never be changed, the whole per-method CloudWatch metric surface (metrics for the enabled method only) is unreachable.
Fix: the UpdateStage handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_stage.rs. Open issue 20261023T125454Z-apigateway-updatestage-stub.md names that file but does not carry this replay key.
Traces: apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStage operation: operation UpdateStage is scaffolded but not implemented in this emulator. Expected: HTTP 200 with the updated Stage, methodSettings carrying metricsEnabled=true for the single enabled method.

## Steps to Reproduce
1. create_rest_api, add a method with a MOCK integration, create_deployment(stageName='prod').
2. update_stage(restApiId=..., stageName='prod', patchOperations=[{'op':'replace','path':'/~1/GET/metricsEnabled','value':'true'}]).
3. Observe HTTP 501 NotImplemented. Or replay apigateway__cloudwatch.obs-apigateway-3-per-method-metrics-only-for-the-enabled-method.
