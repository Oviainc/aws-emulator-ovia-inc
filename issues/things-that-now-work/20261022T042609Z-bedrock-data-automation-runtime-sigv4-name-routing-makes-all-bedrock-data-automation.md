Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-runtime-async-invoke-status-tag-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-runtime-async-invoke-status-tag-lifecycle bedrock-data-automation-runtime-invoke-async-validation-errors bedrock-data-automation-runtime-get-status-resource-not-found bedrock-data-automation-runtime-get-status-validation-error bedrock-data-automation-runtime-sync-invoke-inline-bytes bedrock-data-automation-runtime-sync-invoke-s3-input-output bedrock-data-automation-runtime-tag-operations-resource-not-found bedrock-data-automation-runtime-tag-resource-pattern-validation-error
Issue-Shape-Id: com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomationAsync|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomationAsync|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomationAsync|4xx->5xx|status
Issue-Key: bedrock-data-automation-runtime|GetDataAutomationStatus|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|GetDataAutomationStatus|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|GetDataAutomationStatus|4xx->5xx|status
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomation|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomation|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|InvokeDataAutomation|4xx->5xx|status
Issue-Key: bedrock-data-automation-runtime|TagResource|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|TagResource|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|TagResource|4xx->5xx|status
Issue-Key: bedrock-data-automation-runtime|ListTagsForResource|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|ListTagsForResource|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|ListTagsForResource|4xx->5xx|status
Issue-Key: bedrock-data-automation-runtime|UntagResource|4xx->5xx|$.__type
Issue-Key: bedrock-data-automation-runtime|UntagResource|4xx->5xx|$.message
Issue-Key: bedrock-data-automation-runtime|UntagResource|4xx->5xx|status
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# SigV4-name routing makes all bedrock-data-automation-runtime calls 500 AmbiguousService

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeDataAutomationAsync

## Description
Every one of the six operations bound to AmazonBedrockKeystoneRuntimeService (sdkId "Bedrock Data Automation Runtime", awsJson1_1, apiVersion 2024-06-13) is answered with HTTP 500 and a non-modeled error shape `{"__type":"AmbiguousService"}` instead of being dispatched to the service's handlers at all. The emulator's front-door router is resolving the target service from the SigV4 credential-scope name rather than from the service's own endpoint identity, and the whole Bedrock family shares that name: Smithy service_info for this model gives sigv4Name "bedrock" and arnNamespace "bedrock", while endpointPrefix is "bedrock-data-automation-runtime". The emulator's own error text confirms the collision -- "6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one" -- which matches the six bedrock-* crates in the workspace (bedrock, bedrockagent, bedrockagentcore, bedrockagentcorecontrol, bedrockagentruntime, bedrockdataautomationruntime, bedrockruntime). Because dispatch fails before any handler runs, NO operation of this service is reachable and no modeled response or error can ever be produced. The six causes below are the six distinct operation/response clusters the corpus proves unreachable; they are one shared routing fault, but each must be re-replayed and seen to pass before this issue is closed.

Cause 1 of 6: InvokeDataAutomationAsync, validation rejection.
  AWS: awsJson1_1. For a request that supplies neither a data-automation project nor inline blueprints, the recorded response is HTTP 400 with __type ValidationException (Smithy: ValidationException is a service-common error of AmazonBedrockKeystoneRuntimeService, errorKind client, @httpError 400, doc "This exception will be thrown when customer provided invalid parameters"). The recorded message "At least one of project or inline blueprints must be provided in the request." is an example of a request-derived validation message; the requirement is the 400 status and the ValidationException code.
  Emulator: HTTP 500 with __type AmbiguousService and the router's collision text.
  Why: a client submitting a malformed async invocation gets an unmodeled server fault. SDK error handling never sees ValidationException, so it classifies the failure as retryable server-side and retries a request that can never succeed, instead of surfacing a non-retryable client error to the caller.
  Fix: the front-door service router / service-registry dispatch key -- resolve by endpointPrefix (and the awsJson1_1 X-Amz-Target service id) instead of sigv4Name, so bedrock-family services that share the signing name are still distinguishable. Nothing in this service's own handlers or serializers is at fault.
  Traces: bedrock-data-automation-runtime-async-invoke-status-tag-lifecycle#task#1; also bedrock-data-automation-runtime-invoke-async-validation-errors#task#1, #task#2, #task#3.

Cause 2 of 6: GetDataAutomationStatus, unknown invocation ARN.
  AWS: awsJson1_1. For an invocation ARN that does not exist the recorded response is HTTP 400 with __type ResourceNotFoundException (Smithy: GetDataAutomationStatus declares error ResourceNotFoundException, errorKind client, doc "This exception will be thrown when resource provided from customer not found"; the model's @httpError on that shape is 404, but the recorded wire status for this service is 400, so 400 + ResourceNotFoundException is the requirement). Recorded message, as an example: "Data Automation Invocation ARN not found.".
  Emulator: HTTP 500 with __type AmbiguousService.
  Why: a poller cannot distinguish "this invocation does not exist" from "the service is broken"; it retries a 500 forever rather than failing fast on a client error.
  Fix: same router dispatch key as Cause 1.
  Traces: bedrock-data-automation-runtime-get-status-resource-not-found#task#1; also bedrock-data-automation-runtime-get-status-validation-error#task#1.

Cause 3 of 6: InvokeDataAutomation (sync), validation rejection.
  AWS: awsJson1_1. A sync invocation that names neither a project nor inline blueprints is rejected with HTTP 400 and __type ValidationException (Smithy: ValidationException, service-common, client error, @httpError 400). Recorded message, as an example: "At least one of project or inline blueprints must be provided in the request.".
  Emulator: HTTP 500 with __type AmbiguousService.
  Why: the sync invoke path is entirely unusable -- both the S3-input/output and the inline-bytes forms fail at dispatch, so a client can never exercise the operation and never receives its modeled validation contract.
  Fix: same router dispatch key as Cause 1.
  Traces: bedrock-data-automation-runtime-sync-invoke-inline-bytes#task#1; also bedrock-data-automation-runtime-sync-invoke-s3-input-output#task#1.

Cause 4 of 6: TagResource against an unknown resource ARN.
  AWS: awsJson1_1. Tagging a resource ARN that does not exist yields HTTP 400 with __type ResourceNotFoundException (Smithy: TagResource declares errors ResourceNotFoundException and ServiceQuotaExceededException; ResourceNotFoundException is a client error). Recorded message, as an example: "Data Automation Invocation ARN not found.".
  Emulator: HTTP 500 with __type AmbiguousService.
  Why: tagging workflows cannot tell a bad ARN from an outage, and a malformed/absent ARN is retried as a server fault.
  Fix: same router dispatch key as Cause 1.
  Traces: bedrock-data-automation-runtime-tag-operations-resource-not-found#task#1; also bedrock-data-automation-runtime-tag-resource-pattern-validation-error#task#1.

Cause 5 of 6: ListTagsForResource against an unknown resource ARN.
  AWS: awsJson1_1. HTTP 400 with __type ResourceNotFoundException (Smithy: ListTagsForResource declares error ResourceNotFoundException, client error). Recorded message, as an example: "Data Automation Invocation ARN not found.".
  Emulator: HTTP 500 with __type AmbiguousService.
  Why: tag read-back after a tag/untag cycle is impossible, so clients cannot verify tag state at all.
  Fix: same router dispatch key as Cause 1.
  Traces: bedrock-data-automation-runtime-tag-operations-resource-not-found#task#2; also bedrock-data-automation-runtime-tag-resource-pattern-validation-error#task#2.

Cause 6 of 6: UntagResource against an unknown resource ARN.
  AWS: awsJson1_1. HTTP 400 with __type ResourceNotFoundException (Smithy: UntagResource declares error ResourceNotFoundException, client error). Recorded message, as an example: "Data Automation Invocation ARN not found.".
  Emulator: HTTP 500 with __type AmbiguousService.
  Why: untagging never reports the modeled client error, so cleanup logic keyed on ResourceNotFoundException loops on a 500.
  Fix: same router dispatch key as Cause 1.
  Traces: bedrock-data-automation-runtime-tag-operations-resource-not-found#task#3; also bedrock-data-automation-runtime-tag-resource-pattern-validation-error#task#3.

Scope note: the service IS bootstrapped (bootstrap issue 20261021T211817Z-bootstrap-service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice.md is resolved), so this is a fidelity defect in dispatch, not a missing bootstrap. No open issue carries any of these replay keys. A separate, still-open bootstrap issue (20261021T215320Z-bootstrap-service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.md) owns the sibling bedrock-data-automation control-plane service and is NOT this issue.

## Evidence
Replay of simparity-wire-aws-v1 (9 fixtures, 27 steps, 1 passed, score 3.7%, state_reset_failures 0, bootstrap_signal false) produced 6 divergence clusters for service bedrock-data-automation-runtime, all with the identical actual response.

Expected (recorded AWS wire) vs actual (emulator), per cluster:
1. InvokeDataAutomationAsync -- expected 400 / $.__type="ValidationException" / $.message="At least one of project or inline blueprints must be provided in the request."; got 500 / $.__type="AmbiguousService" / $.message="6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one". divergence_key service=bedrock-data-automation-runtime|InvokeDataAutomationAsync|status=400->500|keys=$.__type,$.message,status. affected_count 4.
2. GetDataAutomationStatus -- expected 400 / "ResourceNotFoundException" / "Data Automation Invocation ARN not found."; got 500 / "AmbiguousService" / same router text. affected_count 2.
3. InvokeDataAutomation -- expected 400 / "ValidationException" / "At least one of project or inline blueprints must be provided in the request."; got 500 / "AmbiguousService". affected_count 2.
4. TagResource -- expected 400 / "ResourceNotFoundException" / "Data Automation Invocation ARN not found."; got 500 / "AmbiguousService". affected_count 2.
5. ListTagsForResource -- expected 400 / "ResourceNotFoundException"; got 500 / "AmbiguousService". affected_count 2.
6. UntagResource -- expected 400 / "ResourceNotFoundException"; got 500 / "AmbiguousService". affected_count 2.

In every cluster the status and the error code both diverge, and the actual body is a router diagnostic rather than any shape in the service's model -- i.e. the response is produced before operation dispatch. Smithy grounding: services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/smithy-model/com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice.json -- awsService.endpointPrefix="bedrock-data-automation-runtime", awsService.arnNamespace="bedrock", sigv4Name="bedrock", protocols=[awsJson1_1]; common_errors include ValidationException (client, @httpError 400) and InternalServerException (server, @httpError 500); ResourceNotFoundException is a client error. AmbiguousService appears nowhere in the model.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout (/__admin/health answers 200; data listener accepts).
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Representative step: fixture bedrock-data-automation-runtime-async-invoke-status-tag-lifecycle, step task#1 -- an awsJson1_1 POST signed with credential scope service "bedrock", X-Amz-Target AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync, carrying neither a project ARN nor inline blueprints. Observe HTTP 500 with {"__type":"AmbiguousService"} instead of HTTP 400 {"__type":"ValidationException"}.
4. Re-replay each bundled cause to confirm the fix closes all six: bedrock-data-automation-runtime-get-status-resource-not-found#task#1 (GetDataAutomationStatus), bedrock-data-automation-runtime-sync-invoke-inline-bytes#task#1 (InvokeDataAutomation), bedrock-data-automation-runtime-tag-operations-resource-not-found#task#1 (TagResource), #task#2 (ListTagsForResource), #task#3 (UntagResource).
5. Also re-replay the remaining cluster members: bedrock-data-automation-runtime-invoke-async-validation-errors#task#1/#task#2/#task#3, bedrock-data-automation-runtime-get-status-validation-error#task#1, bedrock-data-automation-runtime-sync-invoke-s3-input-output#task#1, bedrock-data-automation-runtime-tag-resource-pattern-validation-error#task#1/#task#2/#task#3.
6. Expected after fix: each step is dispatched to the bedrock-data-automation-runtime handlers and returns the recorded status and modeled error code; no response carries __type AmbiguousService.

## Resolution

Fixed: the service's `can_handle` now claims through the shared Bedrock-family rule
(`service_common::bedrock::claims_request` with this model's own service shape id), so an
`awsJson1_1` request naming `AmazonBedrockKeystoneRuntimeService` on the root path reaches
these handlers instead of being refused as `500 AmbiguousService`, and all six operations
answer their modelled contracts (400 `ValidationException` for a submission naming no
extraction target, 400 `ResourceNotFoundException` for an ARN naming no stored invocation,
200 for an accepted submission; the synchronous `InvokeDataAutomation` keeps an honest 501
for the extraction itself). `replay_issue_units` answers `passed` — 8 fixtures / 14 steps,
all 18 `Issue-Key`s `fixed` — and the behaviour is pinned by
`core/router/tests/all/bedrock_signing_name_routing.rs` (every operation dispatched with
the whole family mounted) and the crate's own
`tests/all/data_automation_invocations.rs`.


