Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.batch-put-property-values-partial-entry-errors iottwinmaker.component-type-create-reaches-active-state iottwinmaker.delete-component-type-referenced-by-entity-rejected iottwinmaker.get-property-value-history-nonexistent-entity-not-found iottwinmaker.too-many-tags-exceeded-rejected
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker.batch-put-property-values-partial-entry-errors|_client_error#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: iottwinmaker.component-type-create-reaches-active-state|run#17|AssertionError|assert not_found_message == expected, f"expected message {expected!r}, got {not_found_message!r}"
Issue-Key: iottwinmaker.delete-component-type-referenced-by-entity-rejected|run#13|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Key: iottwinmaker.get-property-value-history-nonexistent-entity-not-found|run#22|AssertionError|assert err2["Code"] == "ValidationException", f"unexpected error={err2!r}"
Issue-Key: iottwinmaker.too-many-tags-exceeded-rejected|run#8|AssertionError|assert error["Code"] == "ServiceQuotaExceededException", f"unexpected code={error['Code']!r}"
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# IoT TwinMaker refusals diverge in class and wording: BatchPutPropertyValues answers 200 instead of a 4xx, component-type not-found/in-use messages are re-worded, a missing component is a 404 instead of a ValidationException, and the per-resource tag quota raises TooManyTagsException instead of ServiceQuotaExceededException

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutPropertyValues

## Description
Five distinct root causes in the iottwinmaker emulator, all about how a refusal is classified and worded. Each cause is independent and must be closed separately; the failing check ids are listed per cause.

=== Cause 1 of 5: BatchPutPropertyValues reports reference errors as a 200 + errorEntries partial result instead of failing the whole call with a 4xx ===
AWS: BatchPutPropertyValues validates every entries[].entityPropertyReference synchronously, before any value is written, and fails the ENTIRE call at the HTTP level as soon as any reference is unresolvable — it never degrades to a per-entry errorEntries partial result for a bad reference. The model backs this: Smithy BatchPutPropertyValues errors = ValidationException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500); http {POST /workspaces/{workspaceId}/entity-properties, code 200}. Recorded behavior distinguishes three reference classes: (a) a well-formed reference to a real ACTIVE entity/component/property on a user-defined component type that declares no write connector -> 400 ValidationException whose message names the missing connector (AWS echoes its internal connector request, so the only fixed part of that message is that it names the connector); (b) a componentName that does not exist on the entity -> 400 ValidationException with the fixed text "Component <componentName> of Entity <entityId> in workspace <workspaceId> not found.", where componentName/entityId/workspaceId are taken from the request; (c) an entityId that does not exist at all -> 404 ResourceNotFoundException. A batch mixing (b) and (c) is still one HTTP-level failure.
Emulator: answers HTTP 200 with a body {"errorEntries": [{"errors": [{"errorCode": "ValidationException", "errorMessage": "Status is not marked as a time-series property.", "entry": {...}}]}]} for a fully well-formed reference — i.e. it both chooses the wrong transport (200 partial result instead of a modelled 4xx error) and invents a reason ("not marked as a time-series property") that AWS does not give for this class; the connector-specific rejection is absent entirely.
Why: a client that checks only for a ClientError sees the call as a success and believes its property values were accepted; it never learns the reference was unwritable. Clients that DO inspect errorEntries get an error code/message pair AWS never emits for this input, and the entity/component/property distinction (ValidationException vs ResourceNotFoundException) that callers branch on is lost.
Fix: the BatchPutPropertyValues handler's reference-resolution/validation stage — resolve entity, component and property definition (and the component type's write connector) up front and return the modelled ValidationException/ResourceNotFoundException for the whole request, reserving errorEntries for per-entry failures that AWS actually reports that way. Related but NOT the same cause: open issue 20261029T090255Z-iottwinmaker-batchputpropertyvalues-wrong-behavior.md (0-length errorEntries violating @length(1..10)) and 20261029T035851Z-iottwinmaker-batchputpropertyvalues-error-response.md (errorMessage exceeding its 256-char bound) — both describe the shape of errorEntries, not the fact that a reference error is served as a 200 at all.
Traces: iottwinmaker.batch-put-property-values-partial-entry-errors

=== Cause 2 of 5: the component-type ResourceNotFoundException message is re-worded ===
AWS: a read or mutation naming a component type that does not exist in the workspace (GetComponentType, UpdateComponentType — including after a successful DeleteComponentType) is refused with ResourceNotFoundException (Smithy: ResourceNotFoundException, httpError 404, message @length 0..2048) whose message is the fixed text "Cannot find component type <componentTypeId> in workspace <workspaceId>." — fixed words "Cannot find component type ", " in workspace " and the trailing period, with componentTypeId and workspaceId taken verbatim from the request.
Emulator: the error CODE is right (ResourceNotFoundException) but the message is a different sentence with a different word order and a different noun: "ComponentType <componentTypeId> in workspace <workspaceId> not found."
Why: operators and tests that match on the refusal text (log scrapers, runbooks, assertions pinned to AWS wording) do not match, and the message no longer reads as the service's own. The same wrong template is served by both GetComponentType and UpdateComponentType, so it is one shared message builder.
Fix: the shared component-type lookup/not-found path used by the component-type handlers (GetComponentType / UpdateComponentType / DeleteComponentType) — emit AWS's "Cannot find component type {id} in workspace {ws}." template.
Traces: iottwinmaker.component-type-create-reaches-active-state

=== Cause 3 of 5: the DeleteComponentType in-use refusal message is re-worded and enumerates internal detail ===
AWS: DeleteComponentType on a component type still referenced by a live entity is refused with ValidationException (Smithy: ValidationException, httpError 400, message @length 0..2048) and the component type stays ACTIVE. The message's fixed text is "Component type " + " is being used by entities [" + "], please delete the entities first.", with the componentTypeId from the request and the bracketed list being the referencing entity ids from the service's state (comma-separated when there is more than one). It names entities only — not the workspace, not a reference count, not the component name on the entity.
Emulator: serves a different sentence that adds state detail AWS does not expose: "ComponentType <componentTypeId> in workspace <workspaceId> is still referenced by <n>: entity <entityId> component <componentName>" (no trailing period, no "please delete the entities first" remediation, a reference count and per-reference component names instead of the entity-id list).
Why: a client cannot parse out the blocking entity ids from the bracketed list AWS documents, so automated remediation ("delete these entities, then retry") cannot be driven off the refusal; the message also leaks per-component reference structure that AWS keeps internal. Related but distinct: open issue 20261029T035851Z-iottwinmaker-deletecomponenttype-error-response.md covers that this same message can run past its 2048-char bound; it does not cover the wording/content being wrong in the first place.
Fix: the DeleteComponentType handler's in-use validation, where the ValidationException message is built.
Traces: iottwinmaker.delete-component-type-referenced-by-entity-rejected

=== Cause 4 of 5: GetPropertyValueHistory misclassifies a missing component as ResourceNotFoundException instead of ValidationException ===
AWS: GetPropertyValueHistory resolves the reference up front and distinguishes the two bad-reference classes (Smithy errors include both ValidationException 400 and ResourceNotFoundException 404). An entityId that does not exist in the workspace -> 404 ResourceNotFoundException "Entity <entityId> in workspace <workspaceId> not found." (the emulator already gets this one right). A REAL entity with a componentName it does not have -> 400 ValidationException with the fixed text "Component <componentName> of Entity <entityId> in workspace <workspaceId> not found.", the three values taken from the request.
Emulator: answers the missing-component case with ResourceNotFoundException (404) and a lower-cased, re-ordered message "entity <entityId> of workspace <workspaceId> has no component <componentName>" — so both the error class (404 ResourceNotFoundException instead of 400 ValidationException) and the message template diverge, and the entity-missing vs component-missing distinction collapses into one code.
Why: a client cannot tell "this entity does not exist" (not retryable, re-resolve the entity) from "this component name is wrong on an existing entity" (a request-shape bug) — both arrive as 404 ResourceNotFoundException. Retry/backoff policies keyed on 400 vs 404 take the wrong branch.
Fix: the GetPropertyValueHistory handler's entity/component reference resolution and its error mapping (shared with GetPropertyValue's reference resolution, per open issue 20261028T180932Z-iottwinmaker-getpropertyvalue-maintainability.md, so fix it in the shared resolver).
Traces: iottwinmaker.get-property-value-history-nonexistent-entity-not-found

=== Cause 5 of 5: TagResource past the per-resource tag quota raises TooManyTagsException instead of ServiceQuotaExceededException ===
AWS: TagResource that would push a resource's CUMULATIVE tag count over the 50-tags-per-resource quota is refused with ServiceQuotaExceededException (Smithy: ServiceQuotaExceededException, httpError 402, message @length 0..2048) and the rejected batch is not persisted; recorded behavior shows this refusal carries an EMPTY message (""). This is the quota refusal, distinct from the modelled per-call shape bound on TagResource.tags (@length 0..50) whose violation is TooManyTagsException (httpError 400) — the request here is only 15 tags, well inside the per-call bound, so the per-call error does not apply.
Emulator: refuses with error code "TooManyTagsException" (400), i.e. it treats a cumulative-quota breach as a per-call shape breach.
Why: a client branching on ServiceQuotaExceededException (the "Tags per resource" quota, adjustable=No — the signal to stop tagging or untag first) never sees it, while TooManyTagsException tells it to shrink the request, which cannot help: the request is already inside the per-call limit. Status code also differs (402 vs 400), so transport-level policies diverge too.
Fix: the TagResource handler's tag-quota validation — count existing plus incoming tags and raise ServiceQuotaExceededException (with an empty message) for the per-resource quota, keeping TooManyTagsException for the per-call tags map bound.
Traces: iottwinmaker.too-many-tags-exceeded-rejected

## Evidence
Behavioral suite: iottwinmaker 17/22 (77%), 5 failed, 0 errored, 0 skipped.

Cause 1 — iottwinmaker.batch-put-property-values-partial-entry-errors [mismatch]
  Expected (AWS): the call is REJECTED — 400 ValidationException naming the missing write connector for a well-formed reference; 400 ValidationException "Component <componentName> of Entity <entityId> in workspace <workspaceId> not found." for a component not on the entity; 404 ResourceNotFoundException for an unknown entityId.
  Actual (emulator): HTTPStatusCode 200 with {'errorEntries': [{'errors': [{'errorCode': 'ValidationException', 'errorMessage': 'Status is not marked as a time-series property.', 'entry': {'entityPropertyReference': {'componentName': 'Sensor', 'entityId': <entity-id>, 'propertyName': 'Status'}, 'propertyValues': [...]}}]}]}
  Harness: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {...}

Cause 2 — iottwinmaker.component-type-create-reaches-active-state [mismatch]
  Expected (AWS): ResourceNotFoundException, message 'Cannot find component type <componentTypeId> in workspace <workspaceId>.'
  Actual (emulator): ResourceNotFoundException, message 'ComponentType <componentTypeId> in workspace <workspaceId> not found.'
  Harness: AssertionError: expected message 'Cannot find component type sim2real.tm.sensor.<suffix> in workspace sim2real-tm-ws-<suffix>.', got 'ComponentType sim2real.tm.sensor.<suffix> in workspace sim2real-tm-ws-<suffix> not found.' (the ids are per-run values; the fixed text is what differs)

Cause 3 — iottwinmaker.delete-component-type-referenced-by-entity-rejected [mismatch]
  Expected (AWS): ValidationException, message 'Component type <componentTypeId> is being used by entities [<entityId>], please delete the entities first.'
  Actual (emulator): ValidationException, message 'ComponentType <componentTypeId> in workspace <workspaceId> is still referenced by 1: entity <entityId> component <componentName>'
  Harness: AssertionError on err['Message'] equality (code matched, text did not).

Cause 4 — iottwinmaker.get-property-value-history-nonexistent-entity-not-found [mismatch]
  Expected (AWS) for a real entity + unknown componentName: Code 'ValidationException' (400), Message 'Component <componentName> of Entity <entityId> in workspace <workspaceId> not found.'
  Actual (emulator): {'Code': 'ResourceNotFoundException', 'Message': 'entity <entityId> of workspace <workspaceId> has no component NoSuchComponent'}
  Harness: AssertionError: unexpected error={'Message': 'entity ... has no component NoSuchComponent', 'Code': 'ResourceNotFoundException'}
  (The unknown-entityId half of the same check passed: 404 ResourceNotFoundException 'Entity <id> in workspace <ws> not found.' is already correct.)

Cause 5 — iottwinmaker.too-many-tags-exceeded-rejected [mismatch]
  Expected (AWS): Code 'ServiceQuotaExceededException' (402), Message '' (empty), second batch not persisted.
  Actual (emulator): Code 'TooManyTagsException' (400).
  Harness: AssertionError: unexpected code='TooManyTagsException'
  Request that triggers it: 40 tags accepted, then a 15-tag TagResource (cumulative 55 > 50) — inside the modelled per-call @length(0..50) on tags.

## Steps to Reproduce
Common setup for every cause: create an IoT TwinMaker workspace (CreateWorkspace with an S3 bucket and a service role), then as needed a component type and an entity, waiting for ACTIVE.

Cause 1 (BatchPutPropertyValues):
1. CreateComponentType with propertyDefinitions {"Status": {dataType: {type: STRING}, defaultValue: {stringValue: "OK"}}}; wait ACTIVE.
2. CreateEntity with components {"Sensor": {componentTypeId: <that type>}}; wait ACTIVE.
3. BatchPutPropertyValues(workspaceId, entries=[{entityPropertyReference: {entityId, componentName: "Sensor", propertyName: "Status"}, propertyValues: [{value: {stringValue: "updated"}, time: <now, ISO-8601>}]}]).
   -> AWS: 400 ValidationException naming the missing connector. Emulator: HTTP 200 with errorEntries.
4. Repeat step 3 with componentName "NoSuchComponent" -> AWS: 400 ValidationException "Component NoSuchComponent of Entity <entityId> in workspace <workspaceId> not found."
5. Repeat step 3 with an entityId that does not exist -> AWS: 404 ResourceNotFoundException.

Cause 2 (component-type not-found message):
1. CreateComponentType; wait ACTIVE; DeleteComponentType; wait until removed.
2. GetComponentType(workspaceId, componentTypeId) -> compare the ResourceNotFoundException message to 'Cannot find component type <componentTypeId> in workspace <workspaceId>.'
3. UpdateComponentType(workspaceId, componentTypeId, propertyDefinitions={"Pressure": {dataType: {type: DOUBLE}}}) -> same expected message.

Cause 3 (DeleteComponentType in-use message):
1. CreateComponentType <ct>; wait ACTIVE. CreateEntity with components {"Comp": {componentTypeId: <ct>}}; wait ACTIVE.
2. DeleteComponentType(workspaceId, <ct>) -> compare the ValidationException message to 'Component type <ct> is being used by entities [<entityId>], please delete the entities first.'

Cause 4 (GetPropertyValueHistory component classification):
1. CreateComponentType with propertyDefinitions {"Temperature": {dataType: {type: DOUBLE}, defaultValue: {doubleValue: 21.5}}}; wait ACTIVE. CreateEntity with components {"Sensor": {componentTypeId: <ct>}}; wait ACTIVE.
2. GetPropertyValue(workspaceId, entityId, componentName="Sensor", selectedProperties=["Temperature"]) -> 21.5 (positive control, passes today).
3. GetPropertyValueHistory(workspaceId, entityId=<real entity>, componentName="NoSuchComponent", selectedProperties=["Temperature"], startTime=<now-1d>, endTime=<now>) -> expect 400 ValidationException 'Component NoSuchComponent of Entity <entityId> in workspace <workspaceId> not found.'; the emulator answers 404 ResourceNotFoundException.

Cause 5 (tag quota):
1. GetWorkspace(workspaceId) -> arn.
2. TagResource(resourceARN=arn, tags={40 distinct keys}) -> succeeds.
3. TagResource(resourceARN=arn, tags={15 more distinct keys}) -> expect ServiceQuotaExceededException (402) with an empty message; the emulator answers TooManyTagsException.
4. ListTagsForResource -> none of the 15 rejected keys are present (this part holds).
5. UntagResource 20 of the first batch, replay step 3 -> must now succeed.
