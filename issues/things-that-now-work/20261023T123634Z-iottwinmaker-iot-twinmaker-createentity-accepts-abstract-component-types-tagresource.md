Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.create-entity-rejects-abstract-component-type iottwinmaker.too-many-tags-exceeded-rejected iottwinmaker.create-component-type-duplicate-id-conflict-rejected iottwinmaker.execute-query-invalid-syntax-rejected
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker.create-entity-rejects-abstract-component-type|_client_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: iottwinmaker.too-many-tags-exceeded-rejected|run#8|AssertionError|assert error["Code"] == "ServiceQuotaExceededException", f"unexpected code={error['Code']!r}"
Issue-Key: iottwinmaker.create-component-type-duplicate-id-conflict-rejected|run#10|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Key: iottwinmaker.execute-query-invalid-syntax-rejected|run#11|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# IoT TwinMaker: CreateEntity accepts abstract component types, TagResource tag-quota refusal uses the wrong error code, and two refusal messages use non-AWS constructions (4 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEntity

## Description
Four independent root causes in the emulated IoT TwinMaker control/data plane, all surfaced by suite simparity-behavioral-aws-v1 against the bootstrapped `iottwinmaker` service (crate service-com-amazonaws-iottwinmaker-awsiottwinmaker). Each cause must be closed before submitting. The failures blocked by the UNIMPLEMENTED BatchPutPropertyValues / DeleteComponentType / GetPropertyValue / GetPropertyValueHistory / UpdateComponentType scaffold stubs are already filed for me and are NOT part of this issue; the three failures OWNED by open issue 20261022T145710Z-iottwinmaker-iot-twinmaker-createsyncjob-answers-501-notimplemented-and-deleteentity.md (CreateSyncJob 501, DeleteEntity child message, workspace not-found message) are likewise excluded. Note that issue's causes 2 and 3 are the same KIND of defect as causes 3 and 4 below (refusal messages built by non-AWS constructions) but in different operations, so they do not cover these.

=== Cause 1 of 4: CreateEntity never consults the component-type registry, so an abstract component type is accepted instead of refused ===
AWS: Smithy (model_path services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json) models CreateEntity as POST /workspaces/{workspaceId}/entities, success 200, with ValidationException (HTTP 400) among its errors (also AccessDenied 403, Conflict 409, InternalServer 500, ServiceQuotaExceeded 402, Throttling 429); the input member `components` is a ComponentsMapRequest whose entries carry `componentTypeId`. The model does not state the abstractness rule, so recorded behavior only: a component type whose only property is a time-series property with `isStoredExternally=true` and that declares NO backing dataReader/dataWriter function has no way to serve values and is treated as ABSTRACT; CreateEntity naming such a component type is refused synchronously with HTTP 400, Error.Code `ValidationException`, and NO entity is created (a following ListEntities does not report the requested entityName). The refusal message is built from the request — it names the offending component type id — recorded example: 'Abstract ComponentType sim2real.tm.sensor.<suffix> can not be used for creating a component'. A concrete component type (a plain non-time-series property) on the same workspace is accepted, and so is an entity with no components, so this is specific to abstractness and not a blanket refusal of typed components.
Emulator: the call SUCCEEDS with HTTP 200 and persists the entity (replay returned entityId/arn/creationDateTime with state 'ACTIVE'). The handler stores `componentTypeId` verbatim without resolving it in the workspace's component-type registry at all — its own module doc still declares that gap ("a component's componentTypeId is stored WITHOUT checking that the named component type exists"), which is now stale because CreateComponentType is implemented and `workspace.component_types` exists.
Why: this is an overly-permissive divergence. A client that relies on AWS refusing an unusable component type gets a silently broken entity instead of a 400: its model validation tests pass against the emulator and fail in production, and every later read/write against that component (GetPropertyValue, BatchPutPropertyValues, GetEntity) operates on a component whose type real AWS would never have allowed. An entity that AWS would never have created is also left in the store, so teardown and list assertions diverge.
Fix: the CreateEntity handler's validation — resolve each `components[*].componentTypeId` against the workspace's component-type registry in the service state, refuse an unknown id, and refuse an ABSTRACT one (an externally-stored time-series property with no dataReader/dataWriter function in `functions`) with the modeled ValidationException BEFORE any state is mutated, so no entity is persisted.
Traces: iottwinmaker.create-entity-rejects-abstract-component-type. Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_entity.rs:295-340 (components accepted and stored unvalidated via components_from_request); stale gap note at lines 22-27.

=== Cause 2 of 4: TagResource's cumulative tag-quota refusal answers TooManyTagsException where AWS answers ServiceQuotaExceededException ===
AWS: Smithy models TagResource as POST /tags with `tags` (TagMap, length 0..50) required, and declares errors AccessDeniedException (403), ResourceNotFoundException (404) and TooManyTagsException (400) — ServiceQuotaExceededException is NOT in this operation's modeled error list, though the service models that shape (httpError 402). Recorded behavior: a TagResource call that would push a resource's CUMULATIVE tag count over the 50-tags-per-resource quota is refused with Error.Code `ServiceQuotaExceededException` and an EMPTY message, and none of the rejected batch is persisted; after untagging enough keys to clear room, the identical call succeeds.
Emulator: the cumulative cap is enforced, but the refusal carries Error.Code `TooManyTagsException` with a constructed message ('applying these tags would leave <n> tags on <arn>, past the maximum of <max>'). The check fails at `assert error["Code"] == "ServiceQuotaExceededException"` with `unexpected code='TooManyTagsException'`.
Why: the error code is the contract. Retry/backoff and quota-handling code keyed on ServiceQuotaExceededException (the "Tags per resource" quota, non-adjustable) never fires, and botocore/SDK error-shape matching for TooManyTagsException matches on the emulator but never in production — a caller cannot distinguish a quota refusal from a per-call tag-shape rejection.
Fix: the TagResource handler's quota branch — emit ServiceQuotaExceededException (with an empty message, as the recording shows) for the cumulative-per-resource quota; because the operation's modeled error enum has no such variant, the operation's error channel/serializer must also be extended to carry it. Keep the existing behavior that nothing is persisted on refusal.
Traces: iottwinmaker.too-many-tags-exceeded-rejected. Quota branch: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/tag_resource.rs:186-197; modeled enum at the same file's TagResourceErrorModeled.

=== Cause 3 of 4: CreateComponentType's duplicate-id ConflictException message is not AWS's construction ===
AWS: Smithy models CreateComponentType as POST /workspaces/{workspaceId}/component-types/{componentTypeId} with ConflictException (HTTP 409) among its errors. Recorded behavior: re-creating an existing componentTypeId is refused 409 ConflictException with a message built entirely from request members — it names the component type FIRST, then the workspace, in the form 'Component type <componentTypeId> in workspace <workspaceId> already exists.' The rule is known because every variable part is a request member, so the construction is the requirement and the ids are examples.
Emulator: the code (ConflictException) is correct, but the message is a different construction with a different word order and capitalization, and no trailing period: 'component type <componentTypeId> already exists in workspace <workspaceId>'. Recorded example vs observed: expected 'Component type sim2real.tm.dup.6f9145efc6c5 in workspace sim2real-tm-ws-2d6f36f01ff9 already exists.', got 'component type sim2real.tm.dup.6f9145efc6c5 already exists in workspace sim2real-tm-ws-2d6f36f01ff9'.
Why: callers and tooling that match or parse this refusal (idempotent-provisioning scripts that treat "already exists" as success, log scrapers, assertions in customer test suites) key on AWS's construction and do not match the emulator's.
Fix: the CreateComponentType handler's duplicate-id conflict branch — the message format string.
Traces: iottwinmaker.create-component-type-duplicate-id-conflict-rejected. Emitted at services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_component_type.rs:223-233.

=== Cause 4 of 4: ExecuteQuery's invalid-syntax ValidationException message is not AWS's construction (different prefix, and a 0-based offset instead of 1-based line/column) ===
AWS: Smithy models ExecuteQuery as POST /queries/execution with `queryStatement` required (length 1..1000) and ValidationException (HTTP 400) among its errors. Recorded behavior: a queryStatement that is not valid PartiQL knowledge-graph syntax is refused 400 ValidationException with a message built from the request — it names the position of the offending token as a 1-BASED line and column and quotes the offending token, in the form 'Invalid QueryStatement: at line <L>, column <C>: invalid token, token recognition error at: \'<token>\''. Recorded example for statement 'THIS IS NOT PARTIQL !!! ###': 'Invalid QueryStatement: at line 1, column 21: invalid token, token recognition error at: \'!!\''.
Emulator: the code (ValidationException) is correct, but the message uses its own prefix and a 0-based character offset with no line number and a single-character token: 'the queryStatement is not a valid query: unexpected character \'!\' at position 20' for the same statement — i.e. a different construction, and the reported position is off by one relative to AWS's 1-based column.
Why: the position in this message is the only machine-readable part of the refusal; an editor, console or client that highlights the offending token from the error (or a test that asserts the column) points at the wrong character and cannot find the 'line'/'column' fields at all. A 0-based offset silently disagrees with AWS's 1-based column even where the shape is tolerated.
Fix: the ExecuteQuery handler's parse/plan error path and the lexer's position formatting — the INVALID_STATEMENT prefix and the unexpected_character construction must produce AWS's form (1-based line and column, 'invalid token, token recognition error at: <token>').
Traces: iottwinmaker.execute-query-invalid-syntax-rejected. Prefix const at services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/execute_query.rs:399 (INVALID_STATEMENT = "the queryStatement is not a valid query"), position formatting at the same file's unexpected_character (line 578-580), applied at lines 229-232.

## Evidence
Suite: simparity-behavioral-aws-v1 — iottwinmaker 8/22 passed (36%), 14 failed. 10 of the 14 are excluded here (5 ALREADY FILED against unimplemented scaffold stubs: BatchPutPropertyValues, DeleteComponentType, GetPropertyValue, GetPropertyValueHistory, UpdateComponentType; 3 OWNED by 20261022T145710Z-iottwinmaker-iot-twinmaker-createsyncjob-answers-501-notimplemented-and-deleteentity.md). The 4 remaining failures are the 4 causes above.

Cause 1 — iottwinmaker.create-entity-rejects-abstract-component-type [mismatch]:
  setup: CreateComponentType with propertyDefinitions {"Temperature": {dataType DOUBLE, isTimeSeries true, isStoredExternally true}} and no functions -> ACTIVE.
  call:  CreateEntity(workspaceId=<ws>, entityName=<name>, components={"Sensor": {componentTypeId: <abstract type>}})
  expected: HTTP 400, Error.Code == "ValidationException", message naming the abstract component type (example: 'Abstract ComponentType <id> can not be used for creating a component'); the entityName must NOT appear in ListEntities afterwards.
  actual:   HTTP 200 and the entity was created: {'entityId': 'e3f0b75c-bd16-4e94-800b-b4b25e594e0f', 'arn': 'arn:aws:iottwinmaker:<region>:<account>:workspace/<ws>/entity/e3f0b75c-...', 'state': 'ACTIVE'} (ids/arn are examples).
  assertion: `raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")`
  controls in the same check that PASSED-shape: a concrete component type and a component-less entity are both accepted, so the expected refusal is abstractness-specific.

Cause 2 — iottwinmaker.too-many-tags-exceeded-rejected [mismatch]:
  setup: TagResource(resourceARN=<workspace arn>, tags=40 keys) succeeds; then TagResource with 15 more NEW keys (cumulative 55 > 50).
  expected: Error.Code == "ServiceQuotaExceededException", Error.Message == "" (AWS returns this refusal with no message text), and none of the second batch persisted; after UntagResource of 20 keys the identical call succeeds.
  actual:   Error.Code == "TooManyTagsException" with a constructed message.
  assertion: `assert error["Code"] == "ServiceQuotaExceededException", f"unexpected code={error['Code']!r}"` -> AssertionError: unexpected code='TooManyTagsException'
  Smithy: TagResource errors = {AccessDeniedException 403, ResourceNotFoundException 404, TooManyTagsException 400}; ServiceQuotaExceededException is modeled in the service (httpError 402) but not bound to TagResource.

Cause 3 — iottwinmaker.create-component-type-duplicate-id-conflict-rejected [mismatch]:
  Error.Code == "ConflictException" matched; message diverged.
  expected (AWS construction): 'Component type <componentTypeId> in workspace <workspaceId> already exists.'
    recorded example: 'Component type sim2real.tm.dup.6f9145efc6c5 in workspace sim2real-tm-ws-2d6f36f01ff9 already exists.'
  actual (emulator construction): 'component type <componentTypeId> already exists in workspace <workspaceId>'
    observed example: 'component type sim2real.tm.dup.6f9145efc6c5 already exists in workspace sim2real-tm-ws-2d6f36f01ff9'
  assertion: `assert err["Message"] == expected_message, ...`
  source: src/ops/create_component_type.rs:226-229 format!("component type {} already exists in workspace {}", ...)

Cause 4 — iottwinmaker.execute-query-invalid-syntax-rejected [mismatch]:
  Error.Code == "ValidationException" matched; message diverged.
  call: ExecuteQuery(workspaceId=<ws>, queryStatement="THIS IS NOT PARTIQL !!! ###")
  expected (AWS construction): 'Invalid QueryStatement: at line <L>, column <C>: invalid token, token recognition error at: \'<token>\''
    recorded example: "Invalid QueryStatement: at line 1, column 21: invalid token, token recognition error at: '!!'"
  actual (emulator construction): "the queryStatement is not a valid query: unexpected character '!' at position 20" (0-based offset, no line, single char)
  assertion: `assert err["Message"] == expected_message, ...`
  source: src/ops/execute_query.rs:399 (INVALID_STATEMENT) + src/ops/execute_query.rs:578-580 (unexpected_character)

Smithy consulted (model_path services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json): CreateEntity POST /workspaces/{workspaceId}/entities, 200, errors {AccessDenied 403, Conflict 409, InternalServer 500, ServiceQuotaExceeded 402, Throttling 429, Validation 400}; CreateComponentType POST /workspaces/{workspaceId}/component-types/{componentTypeId}, errors include Conflict 409; TagResource POST /tags, errors {AccessDenied 403, ResourceNotFound 404, TooManyTags 400}; ExecuteQuery POST /queries/execution, errors include Validation 400 and QueryTimeout 400.

## Steps to Reproduce
Boot the emulator and point an iottwinmaker client at it (steps need iottwinmaker + iam + s3 only). In each case CreateWorkspace(<ws>) with a role arn trusting iottwinmaker.amazonaws.com and s3Location arn:aws:s3:::<bucket>.

Cause 1 (CreateEntity accepts an abstract component type):
1. CreateComponentType(workspaceId=<ws>, componentTypeId=<abstract>, propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true, "isStoredExternally": true}}); wait ACTIVE.
2. CreateEntity(workspaceId=<ws>, entityName=<name>, components={"Sensor": {"componentTypeId": <abstract>}}).
   Expected: HTTP 400 ValidationException naming the abstract component type, and <name> absent from ListEntities.
   Actual: HTTP 200, entity created in state ACTIVE.
3. Control: the same call with a concrete component type ({"Label": {"dataType": {"type": "STRING"}}}) must still succeed, as must an entity with no components.
Replay: iottwinmaker.create-entity-rejects-abstract-component-type

Cause 2 (TagResource quota code):
1. arn = GetWorkspace(<ws>)["arn"].
2. TagResource(resourceARN=arn, tags={f"k{i}": f"v{i}" for i in range(40)}) -> 200.
3. TagResource(resourceARN=arn, tags={f"x{i}": f"v{i}" for i in range(15)}).
   Expected: Error.Code "ServiceQuotaExceededException" with an empty message; ListTagsForResource shows none of the x* keys and still shows the k* keys.
   Actual: Error.Code "TooManyTagsException".
4. UntagResource(arn, tagKeys=[f"k{i}" for i in range(20)]) then replay step 3 — it must now succeed and ListTagsForResource must report x0..x14.
Replay: iottwinmaker.too-many-tags-exceeded-rejected

Cause 3 (CreateComponentType duplicate message):
1. CreateComponentType(workspaceId=<ws>, componentTypeId=<id>, propertyDefinitions={"Pressure": {"dataType": {"type": "DOUBLE"}}}); wait ACTIVE.
2. CreateComponentType again with the same <id> and different propertyDefinitions.
   Expected: ConflictException, Message 'Component type <id> in workspace <ws> already exists.'
   Actual: ConflictException, Message 'component type <id> already exists in workspace <ws>'.
3. GetComponentType(<id>) must still show only "Pressure" (this part passes today).
Replay: iottwinmaker.create-component-type-duplicate-id-conflict-rejected

Cause 4 (ExecuteQuery invalid-syntax message):
1. ExecuteQuery(workspaceId=<ws>, queryStatement="THIS IS NOT PARTIQL !!! ###").
   Expected: 400 ValidationException, Message "Invalid QueryStatement: at line 1, column 21: invalid token, token recognition error at: '!!'" (1-based line/column, offending token quoted).
   Actual: 400 ValidationException, Message "the queryStatement is not a valid query: unexpected character '!' at position 20".
2. Control: a well-formed query (SELECT entity FROM EntityGraph MATCH (entity) WHERE entity.entityName = '<name>') must still return the matching entity row.
Replay: iottwinmaker.execute-query-invalid-syntax-rejected
