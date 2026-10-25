Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected|_poll#31|Timeout|time.sleep(interval_s)
Issue-Commit: d7f2082fceb83802a8fa1cf49a5205bc74f19057

# StartCommandExecution's deprecated-command refusal uses its own message template instead of the service's, so the deprecation never becomes observable and the check hangs to timeout

> _Note: this issue was identified against the code base at commit `d7f2082fceb83802a8fa1cf49a5205bc74f19057`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
Cause 1 of 1: iot-jobs-data `StartCommandExecution` refuses an execution of a command the IoT control plane has DEPRECATED with the right status and the right error code, but with a message built by a different rule than the service's, and that message is the only thing that distinguishes this refusal reason from the other ValidationException refusals of the same operation. The behavioral check keys its deprecation-propagation poll on the service's message form, so the deprecation it just made never becomes observable and the check spins until it is killed (`Check timed out after 60.0s`).

Failing check for this cause: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected.

AWS: `StartCommandExecution` (Smithy, com.amazonaws.iotjobsdataplane: POST /command-executions, success 200; input `targetArn` (TargetArn, @length 0..2048, required), `commandArn` (CommandArn, required), `parameters` (CommandExecutionParameterMap, @length min 1), `executionTimeoutSeconds`, `clientToken`; modeled errors ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400). Starting an execution of a command whose control-plane `deprecated` flag is set (iot `UpdateCommand$deprecated`, DeprecationFlag boolean) is refused with error code `ValidationException` and HTTP 400 — NOT the modeled `ResourceNotFoundException` a purely model-driven reading would pick. The refusal message is request-derived: it names the command the request's `commandArn` named and states that the command has been deprecated — recorded behavior (us-west-2, 2026-09-11, per the check's MEASURED notes) is the template `The command <commandId> has been deprecated.` The requirement here is that rule (a ValidationException/400 whose message is that request-derived form naming the deprecated command), exactly as it is for the sibling refusals of the same operation, whose request-derived message rules this emulator already follows: `The command <commandId> was not found.` for an unknown command and `Invalid target arn <value> not supported as a target` for an unparseable target. Those two message rules were themselves filed and fixed for this operation (20261022T071545Z-iot-jobs-data-startcommandexecution-accepts-a-nonexistent-command-and-rejection-messages.md, causes 2 and 3), so the deprecated refusal is the one refusal of this operation still built by its own phrasing.

Emulator: the refusal is `ValidationException`/HTTP 400 (correct), with the message `The command <commandId> is deprecated and cannot be used to start new command executions.` — its own template rather than the service's. services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs builds it in `command_deprecated_message` (around line 264) and raises it from `handle` (`if command.deprecated { return Err(invalid_request(command_deprecated_message(command_name))); }`, around lines 462-463). The surrounding behavior is right: iot `UpdateCommand` does propagate the flag to the shared index (services/service-com-amazonaws-iot-awsiotservice/src/ops/update_command.rs, `registry.lock().set_deprecated(...)`) and the data plane does read it, so the state transition works and only the message form diverges. Because deprecation is asynchronous on real AWS, the check observes it by polling the refusal until its message matches the service's form; with a message that can never match, the poll never terminates and the whole check is killed at the harness's 60s limit (the check's own poll budget is 120s), which is why the verdict is a timeout rather than an assertion on text.

Why: a client cannot tell WHY its start was refused. All of this operation's client-side refusals arrive as the same `ValidationException`/400, so the message is the only discriminator between "no such command", "bad target", "parameters against a static payload" and "deprecated"; a fleet tool or retry wrapper that keys off the service's deprecation wording to stop retrying (or to surface "undeprecate the command") instead sees unrecognized text and treats a permanent, operator-fixable refusal as an unclassified validation failure — in this check's case by retrying until its deadline. Everything downstream of the deprecation leg is also unreachable: the undeprecate/restore leg (`UpdateCommand(deprecated=false)` must make the identical call succeed again with a fresh executionId) and the closing ListCommandExecutions assertion that none of the four refusals created an execution are never exercised. Hypothesis (NOT observed, the check dies before reaching them): those legs may hold, but this issue is not evidence either way.

Fix: the refusal-message construction in `StartCommandExecution`'s request validation — `command_deprecated_message` in services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs must build the service's request-derived form for a deprecated command (naming the command from `commandArn`, as the not-found and invalid-target messages beside it already do), leaving the status (400) and error code (ValidationException) as they are and changing nothing about the deprecation state read.

Traces: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected (the leg after `iot UpdateCommand(commandId=C, deprecated=True)`; the control, unknown-command, malformed-target and static-payload/parameters legs all pass first).

Related open issue (does NOT cover this cause): 20261024T124853Z-iot-jobs-data-startcommandexecution-accepts-parameters-against-a-static-payload-command.md lists this same check id, but for the earlier `parameters`-against-a-static-payload accept. That leg now refuses correctly (ValidationException/400, "The parameters option is unsupported for commands with static payloads."), so the check advances past it and fails at this later, distinct point, which that issue's replay key does not carry (it keys the `_rejection` AssertionError, whereas this failure is a poll timeout). Its closing Hypothesis anticipated the deprecation leg failing by the same accept; what actually happens is narrower — the flag IS read and the request IS refused, only the message rule diverges.

## Evidence
Suite report (simparity-behavioral-ga-prod-v1): iot-jobs-data 4/5 (80%), 1 failed.
  [mismatch] iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected
  Error: Check timed out after 60.0s

The check's deprecation leg is:
  iot.update_command(commandId=command_id, deprecated=True)
  _poll(lambda: _rejection(lambda: jd.start_command_execution(targetArn=thing_arn, commandArn=command_arn)),
        lambda t: t[2] == f"The command {command_id} has been deprecated.",
        f"{command_id} to be refused as deprecated")   # poll budget 120s, harness kills at 60s

Expected (AWS, recorded us-west-2 2026-09-11): HTTP 400, error code ValidationException, message built by the request-derived rule "name the command the request named and state it has been deprecated" — recorded form (example values): "The command sim2real-behav-jd-cmd-<id> has been deprecated."
Actual (emulator): HTTP 400, error code ValidationException (both correct), message "The command sim2real-behav-jd-cmd-<id> is deprecated and cannot be used to start new command executions." The predicate can never hold, so the poll never terminates and the check is killed at 60s.

Localization: the preceding legs of the SAME check pass, which places the failure at the deprecation leg — the positive control start plus GetCommandExecution reaching CREATED, the unknown-commandArn refusal (ValidationException/400, "The command <id> was not found."), the malformed-targetArn refusal (ValidationException/400, "Invalid target arn not-an-arn not supported as a target"), and the static-payload/parameters refusal (ValidationException/400, "The parameters option is unsupported for commands with static payloads."). An unimplemented stub or an accepted call on the deprecation leg would have surfaced as an immediate ClientError assertion or as `_rejection`'s "expected a rejection but the call SUCCEEDED" AssertionError (that AssertionError is not caught by `_poll`, which only swallows ClientError), not as a timeout — so the deprecation IS being enforced and only the message text differs.

Code basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs — `command_deprecated_message` (~line 264) and its use in `handle` (~lines 459-463), sitting between `command_not_found_message` (~line 258) and `STATIC_PAYLOAD_PARAMETERS_MESSAGE` (~line 274), which both reproduce the service's forms exactly. Flag propagation (working): services/service-com-amazonaws-iot-awsiotservice/src/ops/update_command.rs ~lines 211-231 (`registry.lock().set_deprecated(&command_arn, deprecated)`).
Model basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/smithy-model/com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice.json (StartCommandExecution: POST /command-executions, 200; errors ValidationException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ConflictException 409, ThrottlingException 429, InternalServerException 500); services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json (UpdateCommand$deprecated, DeprecationFlag).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Resolve the data-plane host: iot DescribeEndpoint(endpointType="iot:Data-ATS"); point an iot-jobs-data client at it.
3. iot CreateThing(thingName=T); iot DescribeThing(T) -> thingArn.
4. iot CreateCommand(commandId=C, namespace="AWS-IoT", displayName="sim2real reboot", payload={"content": b'{"reboot": true}', "contentType": "application/json"}) -> commandArn.
5. Against the iot-jobs-data endpoint: StartCommandExecution(targetArn=thingArn, commandArn=commandArn) -> succeeds with an executionId (positive control).
6. iot UpdateCommand(commandId=C, deprecated=True).
7. Repeat StartCommandExecution(targetArn=thingArn, commandArn=commandArn). Observe HTTP 400 / ValidationException (correct) but the message "The command C is deprecated and cannot be used to start new command executions." instead of the service's request-derived form, recorded as "The command C has been deprecated."
8. Replay iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected: its deprecation-propagation poll keys on that message, never observes the deprecation, and the check is killed with "Check timed out after 60.0s" — the undeprecate/restore leg and the closing ListCommandExecutions "refusals created no execution" assertion are never reached.
