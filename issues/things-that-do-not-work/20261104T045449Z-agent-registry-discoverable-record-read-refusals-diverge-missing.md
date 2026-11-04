Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found agent-registry-list-discoverable-registry-records-validation-and-not-found agent-registry-search-discoverable-registry-records-validation-and-not-found
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->2xx|$.message
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->2xx|status
Issue-Key: agent-registry|BatchGetDiscoverableRegistryRecord|4xx->4xx|$.message
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->4xx|$.message
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->4xx|$.message
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# agent-registry discoverable-record read refusals diverge: missing-registry answered 200, not-found wording reworded, constraint-violation message templates wrong (6 causes)

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetDiscoverableRegistryRecord

## Description
Six net-new root causes on the Agent Registry discoverable-record read surface, all found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1) against the emulator. Every one is a REFUSAL-contract defect: the emulator either does not refuse at all, or refuses with text AWS never emits. All six live in this one service.

Related open issues, none of which covers these wire steps: 20261104T021341Z-agent-registry-discoverable-record-reads-resourcenotfoundexception-message-wording.md describes the same two defects as Causes 1-3 below but as BEHAVIORAL findings (corpus simparity-behavioral-aws-v1) with no wire replay key, so a fixer cannot re-replay these fixtures from it; Causes 4-6 are not in it at all. 20261029T192216Z-agent-registry-wire-replay-every-request-refused-pre.md owns the AmbiguousService router refusal seen on one other step of this corpus (that step is excluded here); it does not cover any step below, because every step below was dispatched to and answered by the AgentRegistry handlers.

Cause 1 of 6: BatchGetDiscoverableRegistryRecord does not refuse an entry naming a nonexistent registry — it answers 200.
AWS: Smithy (services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json) BatchGetDiscoverableRegistryRecord errors include ResourceNotFoundException (httpError 404); its output carries a required errors list (BatchGetDiscoverableRegistryRecordErrorList) alongside registryRecords. The model does not state the message text, so recorded behavior only: when the entry names a well-formed registry that does not exist, the call is REFUSED with HTTP 404, x-amzn-errortype ResourceNotFoundException and message whose fixed text is "Registry not found: " followed by exactly one variable part — the registry identifier as supplied in the request. The per-entry errors list is for recordIds missing from a registry that DOES exist, not for a missing registry.
Emulator: answers HTTP 200 with no error at all — no message member and no x-amzn-errortype header in the response (replay: status expected 404, got 200; $.message missing; $headers.x-amzn-errortype missing).
Why: a client cannot distinguish "this registry does not exist" from "this registry exists but has none of these records". Error handling keyed on ResourceNotFoundException never fires, a typo'd or deleted registry id looks like an empty-but-healthy registry, and retry/validation logic built on the 404 degrades to treating the batch as successful.
Fix: the registry-existence check in the BatchGetDiscoverableRegistryRecord handler — resolve the entry's registry before iterating recordIds and return the modeled ResourceNotFoundException when it does not resolve.
Traces: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found#task#1.

Cause 2 of 6: ListDiscoverableRegistryRecords reworded the ResourceNotFoundException message for a well-formed but nonexistent registryId.
AWS: Smithy ListDiscoverableRegistryRecords errors include ResourceNotFoundException (httpError 404); registryId targets RegistryIdentifier (@length 1..2048, @pattern ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$). Message text is recorded behavior only: fixed text "Registry not found: " followed by one variable part, the registry identifier exactly as the caller supplied it.
Emulator: same status (404) and code, but the message is "Registry <registryId> was not found." — the fixed text and the position of the variable part are both wrong (replay: expected 'Registry not found: sim2realnotfnd1', got 'Registry sim2realnotfnd1 was not found.'; the id is the request value, not part of the template).
Why: a client that matches, logs or asserts on the error message, or parses the identifier out of it at a known offset, sees text AWS never produces.
Fix: the ResourceNotFoundException message construction in the ListDiscoverableRegistryRecords handler.
Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#2.

Cause 3 of 6: SearchDiscoverableRegistryRecords reworded the same ResourceNotFoundException message.
AWS: Smithy SearchDiscoverableRegistryRecords errors include ResourceNotFoundException (httpError 404); registryIds targets RegistryIdList (@length 1..1, required). Recorded behavior only: 404 with fixed text "Registry not found: " plus the registry identifier taken from the request's registryIds entry.
Emulator: 404 with "Registry <registryId> was not found." (replay: expected 'Registry not found: sim2realnotfnd1', got 'Registry sim2realnotfnd1 was not found.').
Why: as Cause 2 — the refusal is identifiable by code only, never by message, so message-matching clients and SDK error-shaping tests diverge.
Fix: the ResourceNotFoundException message construction in the SearchDiscoverableRegistryRecords handler.
Traces: agent-registry-search-discoverable-registry-records-validation-and-not-found#task#1.

Cause 4 of 6: the @length constraint-violation message for BatchGetDiscoverableRegistryRecord.entries injects a synthesized "Value '<n> items'" segment AWS omits.
AWS: Smithy BatchGetDiscoverableRegistryRecordRequest.entries targets RegistryRecordsEntryList with @length min 1 max 1, required; violations surface as ValidationException (httpError 400). Recorded behavior for the message: the fixed template "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>" — for a collection member AWS emits NO value segment at all; the only variable parts are the member path ('entries') and the limit from the model (1).
Emulator: status 400 is right, but the message inserts a value segment that renders the collection's size: "1 validation error detected: Value '2 items' at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1" (the '2' is this request's entry count, not part of the template).
Why: AWS's validation-message text is a stable contract that clients, test suites and log parsers key on. A message carrying an extra quoted segment fails exact-match assertions and any regex anchored on "Value at '<member>'", so a client cannot recognise its own constraint violation.
Fix: the constraint-violation message formatter in this operation's request validation (the shared @length violation template) — it must omit the value segment for list/collection members instead of substituting an element count.
Traces: agent-registry-batch-get-discoverable-registry-record-validation-and-not-found#task#2.

Cause 5 of 6: the @pattern constraint-violation message for ListDiscoverableRegistryRecords.registryId both leaks the offending value and quotes the regex with its anchors.
AWS: Smithy registryId targets RegistryIdentifier with @pattern ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$ (an httpLabel member); violation is ValidationException, HTTP 400. Recorded behavior for the message: fixed template "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", with no value segment, and the pattern reproduced WITHOUT the leading ^ and trailing $ — i.e. "(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}". The variable parts are the member path and the model's pattern.
Emulator: 400 with "1 validation error detected: Value '<supplied value>' at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$" (replay supplied 'bad id!'; that string is the request value, the divergence is the extra value segment plus the two anchor characters inside the quoted pattern).
Why: two independent breaks — clients matching the documented template fail, and the pattern they are handed back differs from the one AWS reports, so a client that extracts and re-applies the advertised pattern (for client-side pre-validation) uses a different expression than AWS's.
Fix: the @pattern constraint-violation message formatter in this operation's request validation — drop the value segment and strip the anchors when rendering the pattern.
Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#1.

Cause 6 of 6: the @length constraint-violation message for SearchDiscoverableRegistryRecords.registryIds injects the same synthesized "Value '<n> items'" segment.
AWS: Smithy SearchDiscoverableRegistryRecordsRequest.registryIds targets RegistryIdList with @length min 1 max 1, required; violation is ValidationException, HTTP 400. Recorded behavior: "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1" — no value segment; variable parts are the member path and the model's max.
Emulator: 400 with "1 validation error detected: Value '2 items' at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1".
Why: as Cause 4 — the refusal's message is not the one AWS words for this error class, so message-keyed client handling and conformance assertions diverge on every over-length collection.
Fix: the same collection-@length violation template used by this operation's request validation.
Traces: agent-registry-search-discoverable-registry-records-validation-and-not-found#task#3.

Scope note: the replay of this corpus passed 0 of 14 steps across 3 fixtures; after excluding the single step owned by the router-refusal issue, the 6 causes above account for all 13 remaining diverging steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 14 steps, 0 passed, 14 mismatched). One further diverging step (ListDiscoverableRegistryRecords, 400 -> 500 AmbiguousService) is owned by 20261029T192216Z-agent-registry-wire-replay-every-request-refused-pre.md and is excluded.

Cause 1 — BatchGetDiscoverableRegistryRecord, fixture agent-registry-batch-get-discoverable-registry-record-validation-and-not-found step task#1:
  expected: HTTP 404, x-amzn-errortype ResourceNotFoundException (shape asserted), message "Registry not found: <registryId-from-request>"
  actual:   HTTP 200; $.message missing; $headers.x-amzn-errortype missing
  (replay detail: "status: expected 404, got 200 / $.message: missing (expected 'Registry not found: sim2realnotfnd1', got None) / $headers.x-amzn-errortype: missing")

Cause 2 — ListDiscoverableRegistryRecords, fixture agent-registry-list-discoverable-registry-records-validation-and-not-found step task#2 (status 404 on both sides):
  expected message: "Registry not found: <id>"   (fixed text "Registry not found: " + request id)
  actual message:   "Registry <id> was not found."

Cause 3 — SearchDiscoverableRegistryRecords, fixture agent-registry-search-discoverable-registry-records-validation-and-not-found step task#1 (cluster of 2 steps: task#1, task#2; status 404 on both sides):
  expected message: "Registry not found: <id>"
  actual message:   "Registry <id> was not found."

Cause 4 — BatchGetDiscoverableRegistryRecord, fixture agent-registry-batch-get-discoverable-registry-record-validation-and-not-found step task#2 (cluster of 3 steps: task#2, task#3, task#4; status 400 on both sides):
  expected message: "1 validation error detected: Value at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1"
  actual message:   "1 validation error detected: Value '2 items' at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1"
  fixed text: "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must have length less than or equal to "; variable parts: member path 'entries' (from the request) and the limit 1 (from the model). The emulator's "'2 items'" segment is request-derived and has no counterpart in AWS's template.

Cause 5 — ListDiscoverableRegistryRecords, fixture agent-registry-list-discoverable-registry-records-validation-and-not-found step task#1 (cluster of 3 steps: task#1, task#4, task#5; status 400 on both sides):
  expected message: "1 validation error detected: Value at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}"
  actual message:   "1 validation error detected: Value 'bad id!' at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$"
  fixed text: the "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: " frame; variable parts: member path and the model's pattern rendered WITHOUT ^/$. 'bad id!' is the recording's request value filling the emulator's extra segment, not a requirement.

Cause 6 — SearchDiscoverableRegistryRecords, fixture agent-registry-search-discoverable-registry-records-validation-and-not-found step task#3 (cluster of 3 steps: task#3, task#4, task#5; status 400 on both sides):
  expected message: "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1"
  actual message:   "1 validation error detected: Value '2 items' at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1"

Model references used (query=operation on services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json):
  BatchGetDiscoverableRegistryRecord: POST /discoverable-records-batch; entries -> RegistryRecordsEntryList @length 1..1 required; errors ResourceNotFoundException(404), ValidationException(400).
  ListDiscoverableRegistryRecords: POST /registries/{registryId}/discoverable-records-list; registryId -> RegistryIdentifier @length 1..2048 @pattern ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$; errors ResourceNotFoundException(404), ValidationException(400).
  SearchDiscoverableRegistryRecords: POST /discoverable-records-search; registryIds -> RegistryIdList @length 1..1 required; errors ResourceNotFoundException(404), ValidationException(400).

Code citation for the not-found wording (Causes 2-3): services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:168-177 (message built at :172 as format!("Registry {} was not found.", input.registry_id)); the search handler builds the same sentence.

## Steps to Reproduce
Preconditions: the emulator serving agent-registry, routed so the request reaches the AgentRegistry handlers (not the AmbiguousService refusal), and NO registry present whose id is the well-formed missing id used below (an id matching ^[a-zA-Z0-9]{12,16}$).

Cause 1: POST /discoverable-records-batch (BatchGetDiscoverableRegistryRecord) with entries=[{registryId: <missing-id>, recordIds: [<any-record-id>]}].
  -> expect HTTP 404, x-amzn-errortype ResourceNotFoundException, message "Registry not found: <missing-id>".
  -> emulator answers HTTP 200 with no error member and no errortype header.
  (fixture agent-registry-batch-get-discoverable-registry-record-validation-and-not-found, step task#1)

Cause 2: POST /registries/<missing-id>/discoverable-records-list (ListDiscoverableRegistryRecords).
  -> expect 404 ResourceNotFoundException "Registry not found: <missing-id>"; emulator answers 404 "Registry <missing-id> was not found."
  (same fixture family, step task#2)

Cause 3: POST /discoverable-records-search (SearchDiscoverableRegistryRecords) with searchQuery=<any> and registryIds=[<missing-id>].
  -> same expected/actual message divergence as Cause 2.
  (step task#1 of the search fixture)

Cause 4: POST /discoverable-records-batch with entries containing TWO entries (over the @length max of 1).
  -> expect 400 ValidationException "1 validation error detected: Value at 'entries' failed to satisfy constraint: Member must have length less than or equal to 1"; emulator inserts "Value '2 items' at 'entries'".
  (step task#2 of the batch fixture)

Cause 5: POST /registries/<bad-id>/discoverable-records-list with a registryId that violates the pattern (e.g. a value containing a space and a punctuation character).
  -> expect 400 ValidationException "1 validation error detected: Value at 'registryId' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}"; emulator emits the supplied value in a "Value '<value>' at 'registryId'" segment and quotes the pattern with its ^ and $ anchors.
  (step task#1 of the list fixture)

Cause 6: POST /discoverable-records-search with registryIds containing TWO ids (over the @length max of 1).
  -> expect 400 ValidationException "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1"; emulator inserts "Value '2 items' at 'registryIds'".
  (step task#3 of the search fixture)

Re-replay the three fixtures named in Issue-Trace / Issue-Trace-Ids to confirm all six causes are closed.
