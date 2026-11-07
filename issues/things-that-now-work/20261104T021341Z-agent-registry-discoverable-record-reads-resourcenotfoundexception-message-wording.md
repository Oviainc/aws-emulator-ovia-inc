Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: agent-registry.list-discoverable-registry-records-unauthorized agent-registry.search-discoverable-registry-records-unauthorized agent-registry.batch-get-discoverable-registry-record-id-form-bypasses-authorization
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry.list-discoverable-registry-records-unauthorized|run#9|AssertionError|assert not_found_message == f"Registry not found: {missing_id}", (
Issue-Key: agent-registry.search-discoverable-registry-records-unauthorized|run#10|AssertionError|assert not_found_message == f"Registry not found: {missing_id}", (
Issue-Key: agent-registry.batch-get-discoverable-registry-record-id-form-bypasses-authorization|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 2926e75b84d3dfdb431d7093667b6d3230f0ce6c

# agent-registry discoverable-record reads: ResourceNotFoundException message wording is wrong, and BatchGetDiscoverableRegistryRecord returns 200 for a nonexistent registry instead of 404

> _Note: this issue was identified against the code base at commit `2926e75b84d3dfdb431d7093667b6d3230f0ce6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoverableRegistryRecords

## Description
Two net-new root causes in the Agent Registry discoverable-record read path. Both were found by replaying behavioral checks against the emulator; both live in this one service.

Note: the open issues 20261029T185843Z-agent-registry-is-entirely-unreachable-router-resolves-services.md and 20261029T192216Z-agent-registry-wire-replay-every-request-refused-pre.md describe an AmbiguousService router refusal. Neither covers these: the replayed checks were dispatched and answered by the AgentRegistry handlers (a 200 body and a modeled ResourceNotFoundException came back), so these are fidelity defects in the handlers, not that routing refusal.

Cause 1 of 2: ResourceNotFoundException message for a well-formed but nonexistent registry is worded differently from AWS.
AWS: ListDiscoverableRegistryRecords and SearchDiscoverableRegistryRecords both model ResourceNotFoundException (HTTP 404) — Smithy: ListDiscoverableRegistryRecords errors include ResourceNotFoundException httpError 404, doc "The requested resource was not found."; registryId is RegistryIdentifier @length 1..2048 @pattern ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$. The model does not state the message text, so this is recorded behavior only: the recording shows the message is the fixed text "Registry not found: " followed by one variable part, the registry identifier exactly as the caller supplied it in the request (the plain ID in the recording). Status 404, code ResourceNotFoundException.
Emulator: returns ResourceNotFoundException but with the message "Registry <registryId> was not found." (replay observed: 'Registry sim2realnotfnd1 was not found.'). The fixed text and the position of the variable part are both wrong; only the code and status match.
Why: a client that matches, logs or asserts on the error message — and any code that parses the identifier out of it at a known position — sees text AWS never emits. The two replayed checks fail on exactly this and are therefore unable to reach the authorization assertions that follow, so the whole rejection surface of these two operations is unverifiable while the wording diverges.
Fix: the not-found construction in each operation's handler (the ResourceNotFoundException message formatting), in the discoverable-record read handlers; see Traces for file/line citations.
Traces: agent-registry.list-discoverable-registry-records-unauthorized, agent-registry.search-discoverable-registry-records-unauthorized. Citations: services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:172 and services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:223.

Cause 2 of 2: BatchGetDiscoverableRegistryRecord treats a nonexistent REGISTRY as a per-record error entry instead of failing the request.
AWS: Smithy: BatchGetDiscoverableRegistryRecord models ResourceNotFoundException (httpError 404) among its errors, and its output's errors list (BatchGetDiscoverableRegistryRecordErrorList) carries records that could not be retrieved. The recording distinguishes the two: when the entry names a well-formed registry that does not exist, the call is REFUSED with HTTP 404, code ResourceNotFoundException, message "Registry not found: <registryId>" (fixed text "Registry not found: " plus the registry identifier taken from the request) — the per-entry errors list is for records missing from a registry that DOES exist, not for a missing registry.
Emulator: answers HTTP 200 with registryRecords: [] and one errors entry {registryId, recordId, errorCode: "RESOURCE_NOT_FOUND", message: "No discoverable registry record was found for this identifier."}. No exception is raised at all.
Why: a client cannot tell "this registry does not exist" from "this registry exists but lacks that record". Error handling keyed on ResourceNotFoundException never fires, a typo'd or deleted registry id looks like an empty-but-healthy registry, and retry/validation logic built on the 404 silently degrades to treating the batch as successful.
Fix: the registry-existence check in the BatchGetDiscoverableRegistryRecord handler — resolve the entry's registry BEFORE iterating recordIds and return the modeled ResourceNotFoundException when it does not resolve, reserving the output errors list for records missing inside an existing registry; see Traces for the file/line citation.
Traces: agent-registry.batch-get-discoverable-registry-record-id-form-bypasses-authorization. Citation: services/service-com-amazonaws-agentregistry-agentregistry/src/ops/batch_get_discoverable_registry_record.rs:195-227 (per-record errors push at :214, registry lookup at :201).

## Evidence
Cause 1 — ListDiscoverableRegistryRecords (check agent-registry.list-discoverable-registry-records-unauthorized):
  expected: ResourceNotFoundException, message "Registry not found: sim2realnotfnd1"
  actual:   ResourceNotFoundException, message 'Registry sim2realnotfnd1 was not found.'
  assertion: "unexpected ResourceNotFoundException message: 'Registry sim2realnotfnd1 was not found.'"
  (fixed text differs; the only variable part is the registry identifier from the request)

Cause 1 — SearchDiscoverableRegistryRecords (check agent-registry.search-discoverable-registry-records-unauthorized):
  same expected/actual pair, same assertion, from searchQuery="smoke-test", registryIds=[<missing id>].

Cause 2 — BatchGetDiscoverableRegistryRecord (check agent-registry.batch-get-discoverable-registry-record-id-form-bypasses-authorization):
  expected: the call REJECTED with HTTP 404, Error.Code ResourceNotFoundException, Error.Message "Registry not found: <registryId>"
  actual:   HTTP 200 with body {'registryRecords': [], 'errors': [{'registryId': 'sim2realnotfnd1', 'recordId': 'sim2real0001', 'errorCode': 'RESOURCE_NOT_FOUND', 'message': 'No discoverable registry record was found for this identifier.'}]}
  assertion: "expected the call to be rejected, but it succeeded"

Because both failures land on the positive control at the top of each check, the authorization assertions that follow (AccessDeniedException for an unpermissioned caller, ID-vs-ARN resource normalization, validation-before-authorization) were never evaluated and remain unverified.

## Steps to Reproduce
With the emulator serving agent-registry and NO registry named <missing-id> (a well-formed id matching ^[a-zA-Z0-9]{12,16}$, e.g. a 15-char id) in the store:

Cause 1:
1. agent-registry ListDiscoverableRegistryRecords with registryId=<missing-id>.
   -> expect 404 ResourceNotFoundException "Registry not found: <missing-id>"; emulator returns 404 ResourceNotFoundException "Registry <missing-id> was not found."
2. agent-registry SearchDiscoverableRegistryRecords with searchQuery="smoke-test", registryIds=[<missing-id>].
   -> same divergence.

Cause 2:
3. agent-registry BatchGetDiscoverableRegistryRecord with entries=[{registryId: <missing-id>, recordIds: [<any-record-id>]}].
   -> expect the call refused with 404 ResourceNotFoundException "Registry not found: <missing-id>"; emulator returns HTTP 200 with an empty registryRecords list and one RESOURCE_NOT_FOUND entry in errors.
