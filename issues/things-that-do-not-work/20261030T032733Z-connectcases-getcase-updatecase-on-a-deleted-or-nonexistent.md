Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases.delete-case-removes-case
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases.delete-case-removes-case|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 283a38fd1792cd1319725c85b0b39091ba419fd6

# connectcases: GetCase/UpdateCase on a deleted (or nonexistent) case return the emulator's own ResourceNotFoundException message instead of AWS's "A case with id '<caseId>' could not be found."

> _Note: this issue was identified against the code base at commit `283a38fd1792cd1319725c85b0b39091ba419fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCase

## Description
One net-new root cause found by replaying the behavioral suite against the connectcases emulator. Every other failing check in this run is either ALREADY FILED (blocked by an unimplemented scaffold stub: TagResource, DeleteField, DeleteDomain, DeleteTemplate, SearchRelatedItems) or OWNED by an open issue (20261029T183832Z-connectcases-five-response-refusal-fidelity-defects-getcase-unset.md, 20261029T233519Z-connectcases-createrelateditem-type-content-mismatch-is-refused-with.md) and is skipped here.

=== Cause 1 of 1: the case-not-found refusal message is generated from a generic "<ResourceType> <id> was not found" template, not AWS's sentence for a missing case (failing check: connectcases.delete-case-removes-case) ===

AWS: Smithy: GetCase (POST /domains/{domainId}/cases/{caseId}, @readonly) and UpdateCase (PUT /domains/{domainId}/cases/{caseId}) both model ResourceNotFoundException with httpError 404; the shape's required members are message, resourceId and resourceType, and the shape doc is "We couldn't find the requested resource. Check that your resources exists and were created in the same Amazon Web Services Region as your request, and try your request again." The message TEXT for this error class is recorded behavior only: after a case is deleted (or for a caseId the domain never held), AWS answers HTTP 404 / code ResourceNotFoundException with message `A case with id '<caseId>' could not be found.` The fixed text is `A case with id '` + `' could not be found.`; the only variable part is the caseId, taken verbatim from the request path label (single-quoted). No domain id, no resource-type word and no other request value appears in it. The same operation pair applies the same rule, so UpdateCase on the deleted case must answer the identical code and message.

Emulator: the error CODE and status are right (ResourceNotFoundException / 404) but the message is the emulator's own generic template: `Case <caseId> was not found` — no quoting, the word "Case" as a leading resource-type token, and "was not found" rather than "could not be found." (no trailing period). It comes from the shared helper `not_found(resource_type, resource_id)` in services/service-com-amazonaws-connectcases-amazonconnectcases/src/state.rs, which formats `"{resource_type} {resource_id} was not found"` for EVERY resource class; `Domain::case()` calls it with resource_type "Case", and GetCase/UpdateCase map that straight onto their ResourceNotFoundException. Observed in replay: expected `A case with id '79d54e6c-5cb9-43e6-a4d6-db55d630f915' could not be found.`, got `Case 79d54e6c-5cb9-43e6-a4d6-db55d630f915 was not found` (the id is a per-run value and is not part of the requirement; the fixed text and the single-quoting of the id are).

Why: a client that matches or surfaces the message for this error class — "case was deleted/never existed" handling, UI copy, log-based alerting, contract tests pinned to AWS's wording — never sees the sentence AWS sends, so case-deleted handling silently falls through to a generic error path. Because one shared helper produces the text for all resource classes, the divergence is uniform across the service: it is not a one-off in GetCase.

Fix: the shared ResourceNotFoundException constructor and the case lookups that call it, so the case-not-found class emits AWS's sentence for a missing case rather than a generic resource-type template, and so GetCase and UpdateCase both emit it (the message is the only thing diverging; the 404 and the code already match). Note the interaction with the already-open issue 20261029T183832Z-connectcases-five-response-refusal-fidelity-defects-getcase-unset.md, whose Cause 3 requires the LAYOUT not-found class to emit the fixed sentence `Requested resource not found.`: the two classes need different text, so the single generic helper cannot satisfy both and the message must be chosen per resource class. That issue does not name GetCase/UpdateCase on a deleted case, its check, or this message, which is what this cause records.

Traces: connectcases.delete-case-removes-case (assertion on the GetCase message after DeleteCase; the identical UpdateCase assertion later in the same check is downstream of the failure and was not reached, but is the same cause and the same helper). File/line citations: src/state.rs `not_found()` lines 1565-1571 and `Domain::case()` lines 582-586; src/ops/get_case.rs lines 197-199 map it onto GetCaseErrorModeled::ResourceNotFoundException.

## Evidence
Suite report: connectcases 7/22 (32%), 15 failed. 9 of those are blocked by already-filed unimplemented stubs (TagResource x4, DeleteField, DeleteDomain, DeleteTemplate, SearchRelatedItems x2) and 5 are OWNED by open issues; the one below is the remaining net-new cause.

Cause 1 — connectcases.delete-case-removes-case [mismatch]:
  expected: after DeleteCase, GetCase(domainId, caseId, fields=[{id: <field>}]) -> HTTP 404, Code ResourceNotFoundException, Message == "A case with id '<caseId>' could not be found."
  actual:   Code ResourceNotFoundException (correct), Message == 'Case <caseId> was not found'
  replay line: AssertionError: expected GetCase message "A case with id '79d54e6c-5cb9-43e6-a4d6-db55d630f915' could not be found.", got 'Case 79d54e6c-5cb9-43e6-a4d6-db55d630f915 was not found'
  (the caseId shown is a per-run value; the requirement is the fixed text "A case with id '" ... "' could not be found." with the request's caseId single-quoted inside it)

  same cause, not reached by the replay because it asserts after the above: UpdateCase(domainId, caseId, fields=[...]) on the same deleted case must answer the identical Code and Message; the emulator reaches the same `not_found("Case", id)` helper, so it emits the same diverging text.

  source: services/service-com-amazonaws-connectcases-amazonconnectcases/src/state.rs:1565-1571 formats every not-found message as "{resource_type} {resource_id} was not found"; Domain::case() (lines 582-586) passes resource_type="Case".

Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): GetCase errors include ResourceNotFoundException httpError 404; UpdateCase likewise; ResourceNotFoundException members message/resourceId/resourceType all required.

## Steps to Reproduce
Replay connectcases.delete-case-removes-case against the connectcases emulator (run_behavioral_suite). Manually:
1. CreateDomain(name=<unique>); poll GetDomain until domainStatus != 'CreationInProgress' and assert it is 'Active'.
2. CreateField(domainId, name=<unique>, type='Text') -> fieldId.
3. CreateTemplate(domainId, name=<unique>, status='Active') -> templateId.
4. CreateCase(domainId, templateId, fields=[{id:'title', value:{stringValue:'<any>'}}, {id:<fieldId>, value:{stringValue:'to-delete'}}]) -> caseId.
5. DeleteCase(domainId, caseId).
6. GetCase(domainId, caseId, fields=[{id:<fieldId>}]). Expect HTTP 404, Error.Code == 'ResourceNotFoundException', Error.Message == "A case with id '<caseId>' could not be found.". The emulator returns the right code but Message == 'Case <caseId> was not found'.
7. UpdateCase(domainId, caseId, fields=[{id:<fieldId>, value:{stringValue:'post-delete-update'}}]). Expect the same 404/code and the same message; the emulator again emits 'Case <caseId> was not found'.
