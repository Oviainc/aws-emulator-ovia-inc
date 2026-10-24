Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected|_rejection#26|AssertionError|raise AssertionError(f"expected a rejection but the call SUCCEEDED, returning {result!r}")
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# StartCommandExecution accepts `parameters` against a static-payload command and returns 200 instead of refusing it ValidationException/400

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
Cause 1 of 1: iot-jobs-data `StartCommandExecution` does not reject a request that supplies `parameters` for a command whose payload is STATIC (one created through iot `CreateCommand` with a literal `payload` and no `payloadTemplate`/`mandatoryParameters`). It mints an execution and answers HTTP 200.

Failing check for this cause: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected.

AWS: `StartCommandExecution` (Smithy, com.amazonaws.iotjobsdataplane: POST /command-executions, success 200; input `targetArn` (TargetArn, @length 0..2048, required), `commandArn` (CommandArn, required), `parameters` (CommandExecutionParameterMap, @length min 1), `executionTimeoutSeconds`, `clientToken`; modeled errors ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400). `parameters` is documented as "a list of parameters that are required by the StartCommandExecution API when performing the command on a device" — i.e. it is meaningful only for a command that declares a parameterized payload (iot `CreateCommand` members `payloadTemplate` / `mandatoryParameters`, versus a static `payload` (CommandPayload)). Supplying `parameters` for a static-payload command is therefore not a request the service can honor, and the recorded behavior (us-west-2, 2026-09-11, as the check's MEASURED notes state) is a refusal with error code `ValidationException` and HTTP status 400; the recorded message is `The parameters option is unsupported for commands with static payloads.` — an example of the rule "the request is refused because the referenced command carries a static payload". Note that the refusal is `ValidationException`/400 and NOT the `ResourceNotFoundException` a purely model-driven reading would pick, consistent with the sibling legs of the same check (unknown commandArn, malformed targetArn) which the emulator now gets right.

Emulator: the call SUCCEEDS. Replay output for the check: `AssertionError: expected a rejection but the call SUCCEEDED, returning {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'executionId': '<minted id>'}`. The handler validates `targetArn`'s resource type, validates that `commandArn` names a command that EXISTS (`command_registry(...).lock().contains(&input.command_arn)`), bounds the produced job document's length, and then unconditionally folds whatever `parameters` arrived into the job document (`parameters_json` / `job_document`) and enqueues a record. Nothing in the path reads the referenced command's payload KIND, so a static-payload command and a templated one are treated identically and a request AWS refuses is accepted. Effect-wise this also leaves a live execution behind that the check's closing "the refusals created no execution" assertion would not tolerate.

Why: a client that mistakenly parameterizes a static command gets an `executionId` and a 200 instead of a 400, so the mistake is never surfaced at the API boundary; the parameters it supplied are silently baked into a job document the device's static payload does not use, so the device runs the unparameterized command while the caller believes its parameters were applied. Differential test suites and SDK error-handling paths that expect the modeled `ValidationException` see a success.

Fix: `StartCommandExecution`'s request validation in services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs, which must consult the shared command index (`service_common::iot_commands`, reached via `crate::state::command_registry`) for more than mere existence: the index has to carry the command's payload kind (static `payload` vs `payloadTemplate`/`mandatoryParameters`, written by the iot control plane's `CreateCommand`), and the handler must refuse a request carrying a non-empty `parameters` map against a static-payload command with the modeled `ValidationException` (HTTP 400) before any execution is minted or persisted. Hypothesis (NOT observed in this replay, because the check aborts at the parameters leg before reaching it): the same existence-only lookup also means the command's `deprecated` flag (iot `UpdateCommand$deprecated`, DeprecationFlag boolean) is never read, so the deprecate/undeprecate legs later in the same check will fail the same way once the parameters leg passes — widening the index to carry payload kind and the deprecation flag together addresses both.

Related already-open issue (does NOT cover this cause, different failure point): 20261024T035000Z-iot-jobs-data-command-executions-started-through-iot-jobs-data-startcommandexecution.md lists this same check id, but for the control-plane read-back stall (`GetCommandExecution`/`ListCommandExecutions` answering 501); the check now gets past that read and fails later, on this accept. The fixed issue 20261022T071545Z-iot-jobs-data-startcommandexecution-accepts-a-nonexistent-command-and-rejection-messages.md covered the unknown-command and unsupported-target legs, both of which now pass — this is a net-new, distinct validation gap, not a regression of a refusal.

Traces: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected (the `parameters={"level": {"S": "high"}}` leg, after the control, unknown-command and malformed-target legs have all passed).

## Evidence
Suite report (simparity-behavioral-ga-prod-v1): iot-jobs-data 4/5 (80%), 1 failed.
  [mismatch] iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected
  File "check_iot_jobs_data_start_command_execution_unknown_and_deprecated_command_rejected.py", line 161, in run
    params_code, params_status, params_msg = _rejection(lambda: jd.start_command_execution(targetArn=thing_arn, commandArn=command_arn, parameters={"level": {"S": "high"}}))
  File "...", line 42, in _rejection
    raise AssertionError(f"expected a rejection but the call SUCCEEDED, returning {result!r}")
  AssertionError: expected a rejection but the call SUCCEEDED, returning {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '50', ...}, 'RetryAttempts': 0}, 'executionId': 'a7a357edf87e75a270149203b05d4d38'}

Expected (AWS): HTTP 400, error code ValidationException. Recorded message (example): "The parameters option is unsupported for commands with static payloads." No execution is created by the refusal.
Actual (emulator): HTTP 200 with a freshly minted executionId; an execution record is persisted for the target thing.

Preceding legs of the SAME check pass against the emulator, which localizes the gap: the positive control start + GetCommandExecution(status CREATED), the unknown-commandArn refusal (ValidationException/400, "The command <id> was not found."), and the malformed-targetArn refusal (ValidationException/400, "Invalid target arn not-an-arn not supported as a target"). Only the static-payload/parameters refusal is missing.

Code basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs — `handle` validates targetArn (lines ~421), resolves commandArn and checks existence only (`commands.lock().contains(...)`, lines ~430-436), then serializes `input.parameters` into the job document unconditionally (`parameters_json`/`job_document`, lines ~438-439) with no check of the command's payload kind, and persists the record (lines ~496-517).
Model basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/smithy-model/com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice.json (StartCommandExecution), services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json (CreateCommand: payload / payloadTemplate / mandatoryParameters; UpdateCommand: deprecated).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Resolve the data-plane host: iot DescribeEndpoint(endpointType="iot:Data-ATS"); point an iot-jobs-data client at it.
3. iot CreateThing(thingName=T); iot DescribeThing(T) -> thingArn.
4. iot CreateCommand(commandId=C, namespace="AWS-IoT", displayName="sim2real reboot", payload={"content": b'{"reboot": true}', "contentType": "application/json"}) -> commandArn. Note this command has a STATIC payload: no payloadTemplate and no mandatoryParameters.
5. Against the iot-jobs-data endpoint: StartCommandExecution(targetArn=thingArn, commandArn=commandArn, parameters={"level": {"S": "high"}}).
6. Observe HTTP 200 with an executionId instead of HTTP 400 / error code ValidationException (recorded message example: "The parameters option is unsupported for commands with static payloads.").
7. iot ListCommandExecutions(commandArn=commandArn, startedTimeFilter={"after": "<YYYY-MM-DDThh:mm>"}) -> the refused request's execution is nonetheless present, so the refusal's "no effect" property is violated too.
8. Replay iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected to reproduce end to end.
