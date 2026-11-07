Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-list-discoverable-registry-records-validation-and-not-found
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-list-discoverable-registry-records-validation-and-not-found
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|4xx->4xx|$.message
Issue-Commit: d60110d327db2180b70890b55a3f55e135d42b48

# ListDiscoverableRegistryRecords ValidationException enumerates the filter-name enum value set in the wrong order

> _Note: this issue was identified against the code base at commit `d60110d327db2180b70890b55a3f55e135d42b48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoverableRegistryRecords

## Description
Single cause (Cause 1 of 1): ListDiscoverableRegistryRecords, a filters[] entry whose `name` is not a member of the filter-name enum, HTTP 400 ValidationException — correct status, correct error code, WRONG message text: the enumerated value set inside the message is emitted in a different order than AWS emits it.

AWS: Smithy services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json — ListDiscoverableRegistryRecords (POST /registries/{registryId}/discoverable-records-list), input member `filters` -> RegistryRecordFilterList (@length max 10) of RegistryRecordFilter, whose member `name` is required and targets the enum RegistryRecordFilterName; modeled error ValidationException, errorKind client, httpError 400, members message (required) and reason (default FieldValidationFailed). A filters entry carrying a `name` outside that enum is refused with HTTP 400 ValidationException and `$.message` = "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [descriptorType, recordType]". The fixed text for this error class is "<N> validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; the variable parts are the leading violation count, the member path derived from the request's offending member (here the flattened list-member path for the filters entry that failed, e.g. `filters.<n>.member.name`), and the enum's value set. The value SET comes from the model, but its ORDER is part of the message AWS renders: the recording shows AWS rendering it as `[descriptorType, recordType]` for RegistryRecordFilterName. Hypothesis: AWS renders the enum value set in lexicographic order of the wire values (descriptorType < recordType), not in the model's declaration order, which is [recordType, descriptorType].

Emulator: answers HTTP 400 with the right __type/error code and the right member path, but `$.message` = "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy constraint: Member must satisfy enum value set: [recordType, descriptorType]" — the value set is printed in the Smithy model's declaration order, i.e. the two values are transposed relative to AWS.

Why: the whole message, not just its status and code, is the contract for this error class. A client that surfaces or asserts on AWS's validation text (an SDK-level test, a CLI golden output, a log-scraping or error-classification rule, or a UI that parses the allowed-value list out of the message) sees text that never comes off AWS, so an otherwise correct rejection is reported as a mismatch and any exact-match assertion on the allowed-value list fails. Nothing about the status (400), the code (ValidationException) or the member path diverges — only the ordering inside the bracketed set.

Fix: the shared input-validation message renderer for enum constraints (the component that formats "Member must satisfy enum value set: [...]" from an enum shape's values) — it must emit the value set in the order AWS emits it rather than iterating the generated enum's declaration order. Because the renderer is shared, every enum-constrained member of this service (and of any service using the same formatter) is affected the same way; this corpus only exercises RegistryRecordFilterName. No change to ListDiscoverableRegistryRecords' status, error selection or member-path computation is implied — those already match.

Traces: agent-registry-list-discoverable-registry-records-validation-and-not-found#task#5 (cluster of 1).

Related open issue, which does NOT cover this: 20261029T192216Z-agent-registry-wire-replay-every-request-refused-pre.md filed the pre-dispatch AmbiguousService 500 refusal and its Cause 3 bundled step task#5 into the registryId-pattern cluster while nothing reached a handler. Routing now works and 12 of 14 steps pass; task#5 now reaches the handler, gets the correct 400 ValidationException, and diverges only in this message-ordering detail, which that issue's keys do not describe. The replay report reports this cluster with an empty owned_by.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, fixtures_replayed 3, steps_total 14, steps_passed 12, steps_mismatched 2, steps_blocked 0, steps_errored 0, score_pct 85.7, divergence_count 2.

Divergence cluster (verdict mismatch, divergence_key service=agent-registry|ListDiscoverableRegistryRecords|status=400->400|keys=$.message), fixture agent-registry-list-discoverable-registry-records-validation-and-not-found, step task#5, affected_count 1, stub_path empty, owned_by empty:

  expected_status: 400
  actual_status:   400   (status MATCHES)
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy constraint: Member must satisfy enum value set: [descriptorType, recordType]"
    actual:   "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy constraint: Member must satisfy enum value set: [recordType, descriptorType]"

The ONLY difference is the order of the two values inside the bracketed enum value set. The violation count ("1 validation error detected"), the member path ('filters.1.member.name'), the constraint phrase ("failed to satisfy constraint: Member must satisfy enum value set"), the HTTP status and the error code all agree.

Model evidence for the ordering rule: smithy_query query=closure on ListDiscoverableRegistryRecords reports RegistryRecordFilterName as type enum with values ["recordType", "descriptorType"] — exactly the emulator's output order, and the reverse of what AWS recorded. So the emulator is iterating the model/generated-enum declaration order, while AWS renders [descriptorType, recordType].

Note on the other divergence in this corpus: fixture agent-registry-list-discoverable-registry-records-validation-and-not-found step task#3 still returns HTTP 500 / AmbiguousService instead of 400 ValidationException "Request invalid"; the replay report marks it owned_by 20261029T192216Z-agent-registry-wire-replay-every-request-refused-pre.md, so it is not re-filed here.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from the checkout (default mainline build).
2. Replay the simparity-wire-aws-v1 corpus: 14 steps across the 3 agent-registry fixtures, 12 pass, 2 mismatch.
3. Replay fixture agent-registry-list-discoverable-registry-records-validation-and-not-found step task#5 on its own: POST /registries/{registryId}/discoverable-records-list, sigv4-signed for 'agent-registry', with a well-formed registryId and a `filters` list whose second entry (index 1) carries a `name` value that is NOT a member of RegistryRecordFilterName.
4. Expect: HTTP 400, restJson1 error framing, ValidationException, body `$.message` = "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy constraint: Member must satisfy enum value set: [descriptorType, recordType]".
5. Observe: HTTP 400 ValidationException (correct), but `$.message` ends "...enum value set: [recordType, descriptorType]" — the value set transposed into the model's declaration order.
6. Confirm the source of the ordering: smithy_query(query="closure", operation="ListDiscoverableRegistryRecords", model_path="services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json") shows RegistryRecordFilterName values as ["recordType", "descriptorType"], which is the order the emulator prints, proving the renderer uses declaration order rather than the order AWS renders.
7. After changing the enum-constraint message renderer, re-replay step task#5 of that fixture and assert the full message string, not just the status and error code.
