Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found agent-registry-list-discoverable-registry-records-validation-and-not-found agent-registry-search-discoverable-registry-records-validation-and-not-found
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->5xx|$.message
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->5xx|$headers.content-type
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->5xx|status
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->5xx|$.message
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->5xx|$headers.content-type
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->5xx|status
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->5xx|$.__type
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->5xx|$.message
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->5xx|$headers.content-type
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->5xx|status
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# agent-registry wire replay: every request refused pre-dispatch with AmbiguousService 500 instead of the modeled restJson1 error

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetDiscoverableRegistryRecord

## Description
The wire corpus replayed 14 recorded steps across all 3 AgentRegistry operations and 0 passed. Every single request — success-path and error-path alike — is refused BEFORE dispatch with HTTP 500, error code `AmbiguousService`, content-type `application/x-amz-json-1.1`, and the message "2 emulated services all claim requests for 'agent-registry': agent-registry, agent-registry; the request is rejected rather than served by an arbitrary one". No AgentRegistry handler is ever reached, so each recorded response below is replaced by that one refusal. The shared fault is the request router resolving the target emulated service by SigV4 signing name instead of by the addressed endpoint / HTTP route: AgentRegistry (endpointPrefix `agent-registry`) and AgentRegistryControl (endpointPrefix `agent-registry-control`) both declare sigv4Name `agent-registry`, which is normal and deliberate on AWS, and the duplicated "agent-registry, agent-registry" in the message is the router listing the two crates' shared SIGNING NAME rather than their distinct endpoint prefixes. Two additional framing defects travel with it on every step: `AmbiguousService` is in neither service's modeled error set nor anywhere in AWS's error vocabulary, and the refusal is serialized with awsJson1_1 framing (content-type `application/x-amz-json-1.1`, `x-amzn-errortype: AmbiguousService`) even though both services are restJson1, whose error responses carry content-type `application/json`.

There is one OPEN issue on this router pair, 20261029T185843Z-agent-registry-is-entirely-unreachable-router-resolves-services.md, but it is a BEHAVIORAL finding from simparity-behavioral-aws-v1 and carries only behavioral check keys (agent-registry.list-discoverable-registry-records-unauthorized etc.). It does NOT carry any of the wire replay keys below, so none of these 7 causes is exercised by it. Sibling router issues for other colliding pairs (20261029T185813Z-agent-registry-control-..., 20261029T174910Z-payment-cryptography-control-plane-is-entirely-unreachable-the.md, 20261022T145747Z-connectcampaigns-..., 20261022T110819Z-migrationhub-config-...) name other services and likewise do not carry these keys.

Smithy: services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json — service AgentRegistry, sdkId 'Agent Registry', protocol restJson1, authSchemes [sigv4], sigv4Name 'agent-registry'; 3 bound operations; service-common errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), UnauthorizedException(401), ValidationException(400). ValidationException members: message (required), reason (required, default FieldValidationFailed), fieldList. ResourceNotFoundException member: message (NonBlankString).

--- Cause 1 of 7: BatchGetDiscoverableRegistryRecord, nonexistent registry, 404 ResourceNotFoundException.
AWS: Smithy BatchGetDiscoverableRegistryRecord (POST /discoverable-records-batch) models ResourceNotFoundException errorKind client, httpError 404. A well-formed `entries[].registryId` naming a registry that does not exist is answered HTTP 404, restJson1 error framing (content-type `application/json`, x-amzn-errortype ResourceNotFoundException), body `$.message` = "Registry not found: <registryId>". Fixed text is "Registry not found: "; the variable part is the registryId value taken verbatim from the request (the recording's `sim2realnotfnd1` is one such value, not part of the template).
Emulator: HTTP 500, `$headers.content-type` `application/x-amz-json-1.1`, `$headers.x-amzn-errortype` `AmbiguousService`, `$.message` the AmbiguousService router text.
Why: a client branching on ResourceNotFoundException to create-if-missing never sees it; it gets an unmodeled code at a retryable 5xx status, so it burns its retry budget and then fails with an error no SDK error-shape matches.
Fix: the shared request-router / service-resolution layer (resolve by endpointPrefix or by the candidate services' disjoint restJson1 route tables, treat a shared sigv4Name as expected), plus the pre-dispatch error serializer so a refusal is framed in the resolved service's protocol. No BatchGetDiscoverableRegistryRecord handler change is implied until routing works.
Traces: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found#task#1.

--- Cause 2 of 7: BatchGetDiscoverableRegistryRecord, entries list over its max length, 400 ValidationException.
AWS: Smithy BatchGetDiscoverableRegistryRecord.entries -> RegistryRecordsEntryList, required, @length min 1 max 1. An entries list longer than 1 is refused HTTP 400 ValidationException, content-type `application/json`, `$.message` = "1 validation error detected: Value at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1". All of that text is fixed for this error class except the member path (`entries`) and the bound (`1`), which come from the request's offending member and the model's @length max; the leading count is the number of detected violations.
Emulator: HTTP 500 AmbiguousService as above; no validation runs at all.
Why: the constraint is unenforced on the wire, so a caller that over-fills entries gets a 5xx instead of a deterministic 400 and cannot learn which member or bound it violated.
Fix: router/service-resolution layer first; then BatchGetDiscoverableRegistryRecord input validation must emit the modeled ValidationException.
Traces: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found#task#2 (cluster also covers task#3, task#4).

--- Cause 3 of 7: ListDiscoverableRegistryRecords, registryId pattern violation, 400 ValidationException.
AWS: Smithy ListDiscoverableRegistryRecords.registryId -> RegistryIdentifier, @httpLabel, required, @length 1..2048, @pattern (arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}. A registryId that does not match is refused HTTP 400 ValidationException, content-type `application/json`, `$.message` = "1 validation error detected: Value at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — fixed text as quoted, with the member name from the request and the pattern taken from the model (AWS renders it without the ^ and $ anchors).
Emulator: HTTP 500 AmbiguousService.
Why: malformed identifiers are never rejected with the modeled 400, so a client's input-validation feedback path is replaced by an unmodeled retryable server error.
Fix: router/service-resolution layer; then ListDiscoverableRegistryRecords httpLabel/pattern validation.
Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#1 (cluster also covers task#4, task#5).

--- Cause 4 of 7: ListDiscoverableRegistryRecords, nonexistent registry, 404 ResourceNotFoundException.
AWS: ResourceNotFoundException, httpError 404, content-type `application/json`, `$.message` = "Registry not found: <registryId>" — same template as Cause 1, registryId from the request.
Emulator: HTTP 500 AmbiguousService.
Why: not-found handling never fires on the list path either; the 404/500 difference also flips the error from client-fault to server-fault for every retry classifier.
Fix: router/service-resolution layer; then the ListDiscoverableRegistryRecords handler's registry lookup.
Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#2.

--- Cause 5 of 7: ListDiscoverableRegistryRecords, non-field request rejection, 400 ValidationException with `__type` and a short message.
AWS: recorded behavior — HTTP 400 with `$.__type` = "ValidationException" and `$.message` = "Request invalid", content-type `application/json`. Both strings are fixed text for this rejection class (no request-derived values appear in it); ValidationException is the modeled 400 client error for the operation and its `reason` defaults to FieldValidationFailed.
Emulator: HTTP 500, `$.__type` = "AmbiguousService", the router message, awsJson1_1 content-type. Note the emulator does populate `__type`, so only its VALUE and the status diverge here, which is a direct consequence of the pre-dispatch refusal.
Why: a client cannot distinguish a malformed request from an emulator-internal failure; the error discriminator carries a code that is not in the service model.
Fix: router/service-resolution layer; then the ListDiscoverableRegistryRecords request-rejection path must answer the modeled ValidationException discriminator.
Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#3.

--- Cause 6 of 7: SearchDiscoverableRegistryRecords, nonexistent registry, 404 ResourceNotFoundException.
AWS: Smithy SearchDiscoverableRegistryRecords (POST /discoverable-records-search), ResourceNotFoundException httpError 404; content-type `application/json`, `$.message` = "Registry not found: <registryId>" — the registryId being the offending entry of the request's registryIds list.
Emulator: HTTP 500 AmbiguousService.
Why: a search against a registry that was deleted or mistyped reports a server failure instead of the modeled client 404.
Fix: router/service-resolution layer; then the SearchDiscoverableRegistryRecords handler's registry lookup.
Traces: agent-registry-search-discoverable-registry-records-validation-and-not-found#task#1 (cluster also covers task#2).

--- Cause 7 of 7: SearchDiscoverableRegistryRecords, registryIds list over its max length, 400 ValidationException.
AWS: Smithy SearchDiscoverableRegistryRecords.registryIds -> RegistryIdList, required, @length min 1 max 1. More than one registryId is refused HTTP 400 ValidationException, content-type `application/json`, `$.message` = "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1" — fixed text as quoted, member name from the request, bound from the model's @length max.
Emulator: HTTP 500 AmbiguousService; the single-registry restriction is unenforced.
Why: a caller that fans a search across several registries is told nothing about the one-registry limit and sees a retryable 5xx instead.
Fix: router/service-resolution layer; then SearchDiscoverableRegistryRecords input validation.
Traces: agent-registry-search-discoverable-registry-records-validation-and-not-found#task#3 (cluster also covers task#4, task#5).

All 7 causes must be closed and re-replayed: fixing the router alone makes the handlers reachable, but Causes 2, 3, 5 and 7 additionally require the operations' validation and error serialization to produce the modeled status, code and message text, and Causes 1, 4 and 6 require the not-found lookup and its "Registry not found: <registryId>" wording.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, fixtures_replayed 3, steps_total 14, steps_passed 0, steps_mismatched 14, score_pct 0.0, divergence_count 7. Not one step reached an AgentRegistry handler.

Actual on EVERY step (identical): HTTP 500; `$headers.content-type` = `application/x-amz-json-1.1`; `$headers.x-amzn-errortype` = `AmbiguousService` (and `$.__type` = `AmbiguousService` where the recording asserts the discriminator); `$.message` = "2 emulated services all claim requests for 'agent-registry': agent-registry, agent-registry; the request is rejected rather than served by an arbitrary one".

Expected vs actual, per divergence cluster:
1. BatchGetDiscoverableRegistryRecord — expected 404, got 500; `$.message` expected "Registry not found: <registryId>" (recorded value: 'Registry not found: sim2realnotfnd1'); `$headers.content-type` expected `application/json`. divergence_key service=agent-registry|BatchGetDiscoverableRegistryRecord|status=404->500. Fixture agent-registry-batch-get-discoverable-registry-record-validation-and-not-found, step task#1 (1 step).
2. BatchGetDiscoverableRegistryRecord — expected 400, got 500; `$.message` expected "1 validation error detected: Value at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1"; content-type expected `application/json`. Fixture agent-registry-batch-get-discoverable-registry-record-validation-and-not-found, steps task#2, task#3, task#4 (3 steps).
3. ListDiscoverableRegistryRecords — expected 400, got 500; `$.message` expected "1 validation error detected: Value at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}"; content-type expected `application/json`. Fixture agent-registry-list-discoverable-registry-records-validation-and-not-found, steps task#1, task#4, task#5 (3 steps).
4. ListDiscoverableRegistryRecords — expected 404, got 500; `$.message` expected "Registry not found: <registryId>" (recorded: 'Registry not found: sim2realnotfnd1'). Fixture agent-registry-list-discoverable-registry-records-validation-and-not-found, step task#2 (1 step).
5. ListDiscoverableRegistryRecords — expected 400, got 500; `$.__type` expected `ValidationException`, got `AmbiguousService`; `$.message` expected "Request invalid"; content-type expected `application/json`. Fixture agent-registry-list-discoverable-registry-records-validation-and-not-found, step task#3 (1 step).
6. SearchDiscoverableRegistryRecords — expected 404, got 500; `$.message` expected "Registry not found: <registryId>" (recorded: 'Registry not found: sim2realnotfnd1'). Fixture agent-registry-search-discoverable-registry-records-validation-and-not-found, steps task#1, task#2 (2 steps).
7. SearchDiscoverableRegistryRecords — expected 400, got 500; `$.message` expected "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1"; content-type expected `application/json`. Fixture agent-registry-search-discoverable-registry-records-validation-and-not-found, steps task#3, task#4, task#5 (3 steps).

Model evidence for the collision: AgentRegistry declares sigv4Name 'agent-registry' with awsService.endpointPrefix 'agent-registry'; AgentRegistryControl declares the same sigv4Name 'agent-registry' with endpointPrefix 'agent-registry-control'. Both crates are present and stood up in this build (service-com-amazonaws-agentregistry-agentregistry and service-com-amazonaws-agentregistrycontrol-agentregistrycontrol are both in the bootstrapped set), so the failure is at routing time, not a bootstrap gap. `AmbiguousService` appears in neither service's error list, and both services are restJson1, so no error of theirs is ever framed as `application/x-amz-json-1.1`.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator with both Agent Registry crates present (service-com-amazonaws-agentregistry-agentregistry and service-com-amazonaws-agentregistrycontrol-agentregistrycontrol) — the default mainline build reproduces it.
2. Replay the simparity-wire-aws-v1 corpus. All 14 steps of the 3 agent-registry fixtures mismatch with 0 passes.
3. Cause 1: replay agent-registry-batch-get-discoverable-registry-record-validation-and-not-found step task#1 — POST /discoverable-records-batch signed sigv4 for 'agent-registry' with a single entry whose registryId is well-formed but nonexistent. Expect HTTP 404, content-type application/json, ResourceNotFoundException, message "Registry not found: <that registryId>". Observed: HTTP 500, application/x-amz-json-1.1, x-amzn-errortype AmbiguousService.
4. Cause 2: same fixture, step task#2 (and task#3, task#4) — entries carrying more than one element. Expect HTTP 400 ValidationException "1 validation error detected: Value at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1". Observed: the same AmbiguousService 500.
5. Cause 3: agent-registry-list-discoverable-registry-records-validation-and-not-found step task#1 (and task#4, task#5) — POST /registries/{registryId}/discoverable-records-list with a registryId violating RegistryIdentifier's pattern. Expect HTTP 400 ValidationException naming member 'registryId' and the pattern. Observed: AmbiguousService 500.
6. Cause 4: same fixture, step task#2 — well-formed nonexistent registryId. Expect HTTP 404 "Registry not found: <registryId>". Observed: AmbiguousService 500.
7. Cause 5: same fixture, step task#3 — the recorded non-field rejection. Expect HTTP 400 with __type ValidationException and message "Request invalid". Observed: HTTP 500 with __type AmbiguousService.
8. Cause 6: agent-registry-search-discoverable-registry-records-validation-and-not-found steps task#1, task#2 — POST /discoverable-records-search naming a nonexistent registry in registryIds. Expect HTTP 404 "Registry not found: <registryId>". Observed: AmbiguousService 500.
9. Cause 7: same fixture, steps task#3, task#4, task#5 — registryIds carrying more than one id. Expect HTTP 400 ValidationException "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1". Observed: AmbiguousService 500.
10. Confirm the root cause: both services declare sigv4Name 'agent-registry' while their endpointPrefixes differ ('agent-registry' vs 'agent-registry-control') and their restJson1 route tables are disjoint; resolving the target service by endpoint/route instead of signing name makes both reachable, after which steps 3-9 must be re-replayed individually because each also asserts its own status, error code and message text.
