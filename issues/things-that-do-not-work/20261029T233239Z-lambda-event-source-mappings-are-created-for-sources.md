Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-esm-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-esm-error-paths lambda-event-source-mapping-lifecycle lambda-esm-filter-criteria-invalid-pattern-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateEventSourceMapping|4xx->4xx|$.Type
Issue-Key: lambda|CreateEventSourceMapping|4xx->4xx|$.message
Issue-Key: lambda|CreateEventSourceMapping|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateEventSourceMapping|4xx->4xx|status
Issue-Key: lambda|CreateEventSourceMapping|4xx->2xx|$.Type
Issue-Key: lambda|CreateEventSourceMapping|4xx->2xx|$.message
Issue-Key: lambda|CreateEventSourceMapping|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateEventSourceMapping|4xx->2xx|status
Issue-Key: lambda|GetEventSourceMapping|4xx->4xx|$.Type
Issue-Key: lambda|UpdateEventSourceMapping|4xx->4xx|$.Type
Issue-Key: lambda|DeleteEventSourceMapping|4xx->4xx|$.Type
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.BatchSize
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.EventSourceArn
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.EventSourceMappingArn
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.FilterCriteria
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.FunctionArn
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.FunctionResponseTypes
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.LastModified
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.MaximumBatchingWindowInSeconds
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.State
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.StateTransitionReason
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|$.UUID
Issue-Key: lambda|CreateEventSourceMapping|2xx->4xx|status
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].EventSourceMappingArn
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].FilterCriteria.Filters[*].Pattern
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].FunctionResponseTypes
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].State
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*].UUID
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda event source mappings are created for sources AWS refuses, and their refusals diverge

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Shared fault: CreateEventSourceMapping does not validate the source or the filter criteria before persisting the mapping, and the error paths carry the wrong status/code/message. Nine net-new clusters.

AWS: Smithy CreateEventSourceMapping (POST /2015-03-31/event-source-mappings, 202) binds InvalidParameterValueException (400, {Type, message}), ResourceConflictException (409) and ResourceNotFoundException (404). Recorded behavior: a mapping naming a function that does not exist is refused 400 InvalidParameterValueException with message `Function does not exist` (wholly fixed text) — not 404; a source ARN of an unsupported service is refused 400 with `Unrecognized event source. Unsupported source arn : <eventSourceArn>` (fixed text plus the request's ARN); a mapping whose SQS queue does not exist is refused 400 with `Error occurred while ReceiveMessage. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.` (wholly fixed text); and FilterCriteria whose Pattern is not a valid filter document is refused 400 with `Invalid filter pattern definition.` (wholly fixed text). A created-but-disabled mapping reports `State: "Disabled"` and `FunctionResponseTypes: []`.

Emulator: the function-missing case answers 404 ResourceNotFoundException; the unsupported-source and nonexistent-queue cases are accepted (202) or refused with the emulator's own long enumeration text; the invalid filter pattern is stored verbatim and the operation later fails 409 ResourceConflictException as a duplicate mapping; `Type` is dropped from every refusal; the persisted mapping reports `State: "Creating"`, omits `FunctionResponseTypes`, and ListEventSourceMappings echoes the unvalidated pattern.

Why: a caller that relies on CreateEventSourceMapping to reject a bad source or a malformed filter gets a mapping UUID back and believes the wiring is good; the follow-on 409 then misreports a validation failure as a duplicate, and the stored filter can never match.

Fix: CreateEventSourceMapping's validation must resolve the function (400 `Function does not exist`), classify the source ARN, probe the named queue, and parse FilterCriteria patterns before the mapping is persisted, using AWS's message templates; the mapping serializer must report the Disabled state and FunctionResponseTypes.

Cause 1 of 9: CreateEventSourceMapping — fixture lambda-esm-error-paths step task#5; expected 400 InvalidParameterValueException `Function does not exist`, emulator answered 404 ResourceNotFoundException.
Cause 2 of 9: CreateEventSourceMapping, 400 — fixture lambda-esm-error-paths step task#6; message must be `Unrecognized event source. Unsupported source arn : <arn>`, emulator returned its own enumeration of supported sources; `Type` missing.
Cause 3 of 9: CreateEventSourceMapping — fixture lambda-esm-error-paths step task#7; expected 400 with the SQS NonExistentQueue message, emulator answered 202 and created the mapping.
Cause 4 of 9: GetEventSourceMapping, 404 — fixture lambda-esm-error-paths step task#8; `Type` missing.
Cause 5 of 9: UpdateEventSourceMapping, 404 — fixture lambda-esm-error-paths step task#9; `Type` missing.
Cause 6 of 9: DeleteEventSourceMapping, 404 — fixture lambda-esm-error-paths step task#10; `Type` missing.
Cause 7 of 9: CreateEventSourceMapping — fixture lambda-esm-filter-criteria-invalid-pattern-rejected step task#8; expected 400 `Invalid filter pattern definition.`, emulator answered 409 ResourceConflictException about an already-existing mapping.
Cause 8 of 9: CreateEventSourceMapping — fixture lambda-esm-filter-criteria-invalid-pattern-rejected step task#9; expected 202 with the valid FilterCriteria persisted, emulator answered 409 (consequence of cause 7 leaving the rejected mapping in state).
Cause 9 of 9: ListEventSourceMappings, 200 — fixture lambda-esm-filter-criteria-invalid-pattern-rejected step task#11; the listed mapping carries the invalid pattern, `State: "Creating"` instead of `Disabled`, and omits `FunctionResponseTypes`.

## Evidence
Cluster details: task#5 `status: expected 400, got 404` with `$.message: missing (expected 'Function does not exist')`; task#6 message value_mismatch against `Unrecognized event source. Unsupported source arn : arn:aws:s3:::...`; task#7 `status: expected 400, got 202` with the SQS NonExistentQueue message missing; task#8/9/10 `$.Type: missing`; filter task#8 `status: expected 400, got 409` with `$.message` expected `Invalid filter pattern definition.`; filter task#9 `status: expected 202, got 409`; filter task#11 shows `FilterCriteria.Filters[0].Pattern` = `{not json`, `State` `Creating` vs `Disabled`, `FunctionResponseTypes: missing (expected [])`.

## Steps to Reproduce
Replay the corpus. Fixture lambda-esm-error-paths step task#7: CreateEventSourceMapping against a queue ARN that does not exist — expect 400, observe 202. Fixture lambda-esm-filter-criteria-invalid-pattern-rejected step task#8: CreateEventSourceMapping with FilterCriteria Pattern `{not json` — expect 400 `Invalid filter pattern definition.`, observe 409.
