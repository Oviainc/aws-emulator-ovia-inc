Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-catalog-discovery
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-catalog-discovery workspaces-instances-delete-workspace-instance-not-found workspaces-instances-get-workspace-instance-not-found
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|ListInstanceTypes|4xx->4xx|$.message
Issue-Key: workspaces-instances|ListRegions|4xx->4xx|$.message
Issue-Key: workspaces-instances|DeleteWorkspaceInstance|4xx->4xx|$.Message
Issue-Key: workspaces-instances|DeleteWorkspaceInstance|4xx->4xx|$.ResourceType
Issue-Key: workspaces-instances|DeleteWorkspaceInstance|4xx->4xx|status
Issue-Key: workspaces-instances|GetWorkspaceInstance|4xx->4xx|$.Message
Issue-Key: workspaces-instances|GetWorkspaceInstance|4xx->4xx|$.ResourceType
Issue-Key: workspaces-instances|GetWorkspaceInstance|4xx->4xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# workspaces-instances wire error responses diverge from AWS (validation message echoes value; not-found returns 404 with wrong ResourceType)

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypes

## Description
Replaying the wire corpus against the workspaces-instances (EUCMIFrontendAPIService) emulator shows four error-response divergences, grouped here because they are all defects in how this crate builds its modeled client-error bodies: two in the ValidationException message formatter shared by input validation, and two in the not-found error construction shared by the single-instance operations. FOUR causes must be closed before this issue is submitted.

Cause 1 of 4: ListInstanceTypes MaxResults above the modeled range — validation message reproduces the rejected value.
AWS: ListInstanceTypes.MaxResults targets ListInstanceTypesMaxResults with @range min 1 max 600, and the operation models ValidationException (httpError 400) (Smithy: ListInstanceTypes input member MaxResults; error ValidationException httpError 400). The recorded 400 body carries the message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 600" — the member path and the violated constraint are named and the submitted value is NOT reproduced. The message is a requirement here because the rule that produces it is known: it is built from the request (offending member path + the constraint that member violated); the omission of the value itself is recorded behavior only.
Emulator: returns the correct status (400) and error code, but inserts the rejected value — "1 validation error detected: Value '601' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 600" (recorded request value shown as an example).
Why: a client that diffs or asserts on validation text gets a message whose shape changes with every request, and caller-supplied input is reflected back in an error body where the service does not reflect it (NextToken on this service is modeled @sensitive, so echoing request values into validation text is the wrong default here).
Fix: the shared ValidationException message formatter used by this crate's input validation — render the member path and constraint without the value.
Traces: workspaces-instances-catalog-discovery#task#3.

Cause 2 of 4: ListRegions MaxResults above the modeled range — same formatter defect on a second member/constraint, confirming it is not operation-local.
AWS: ListRegions.MaxResults targets MaxResults with @range min 1 max 25; ValidationException httpError 400 (Smithy: ListRegions input member MaxResults; error ValidationException httpError 400). Recorded 400 message: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25".
Emulator: "1 validation error detected: Value '100' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25" — status 400 and code correct, value echoed.
Why: same as Cause 1; it reproduces on every range-constrained member, so fixing only ListInstanceTypes leaves the divergence in place.
Fix: same shared ValidationException message formatter (one edit should close Causes 1 and 2; re-replay both steps).
Traces: workspaces-instances-catalog-discovery#task#4.

Cause 3 of 4: GetWorkspaceInstance on a well-formed but unknown instance id returns HTTP 404 instead of the recorded 400, and populates ResourceType with an unmodelled token form.
AWS: GetWorkspaceInstance models ResourceNotFoundException with required members Message, ResourceId and ResourceType, all targeting String (Smithy: GetWorkspaceInstance errors include ResourceNotFoundException; shape ResourceNotFoundException members Message/ResourceId/ResourceType: String, doc "Type of the resource that was not found"). The recording for an unknown wsinst- id answers HTTP 400 with the ResourceNotFoundException body and ResourceType "WORKSPACE_INSTANCE" — the status and the ResourceType value are wire requirements (recorded behavior only; the model's httpError on the shape is 404, so the service overrides it on this path). The recorded Message "Workspace instance is not found" is a fixed string whose construction rule is not documented, so it is cited as an example, not asserted as a requirement.
Emulator: answers HTTP 404 and sets ResourceType to "workspace-instance" (lower-case, hyphenated), with Message "WorkSpace Instance <id> does not exist".
Why: a client branching on the HTTP status (400 client-input error vs 404 missing resource) classifies this response differently than against AWS, and a client that switches on the structured ResourceType field never matches "WORKSPACE_INSTANCE", so resource-type-specific not-found handling silently falls through.
Fix: the not-found error construction behind GetWorkspaceInstance's handler/serializer for this crate — emit the recorded status for ResourceNotFoundException on this path and the upper-case underscored ResourceType token.
Traces: workspaces-instances-get-workspace-instance-not-found#task#1.

Cause 4 of 4: DeleteWorkspaceInstance on an unknown instance id has the identical defect, so the fix must cover the shared path and not just Get.
AWS: DeleteWorkspaceInstance models ResourceNotFoundException (Smithy: DeleteWorkspaceInstance errors include ResourceNotFoundException, httpError 404 on the shape); the recording answers HTTP 400 with ResourceNotFoundException and ResourceType "WORKSPACE_INSTANCE" (recorded behavior only). Recorded Message "Workspace instance is not found" is again an example.
Emulator: HTTP 404 with ResourceType "workspace-instance" and Message "WorkSpace Instance <id> does not exist".
Why: same client-visible consequence as Cause 3, on the delete path (a caller treating 404 as already-deleted vs 400 as a rejected request takes a different branch than it would against AWS).
Fix: same not-found construction shared by the single-instance operations (one edit should close Causes 3 and 4; re-replay both steps).
Traces: workspaces-instances-delete-workspace-instance-not-found#task#1.

Related open issue: "ValidationException message construction diverges from AWS for workspaces-instances (echoes rejected value, wrong list-member path, anchored pattern)" (20261022T031023Z-workspaces-instances-validationexception-message-construction-diverges-from-aws-for-workspaces.md) describes the same value-echo formatter defect from the BEHAVIORAL corpus, but carries no wire replay key, so Causes 1 and 2 here are reported with the fixture bindings a wire re-replay needs. Note that issue states the not-found discriminator "passes" with ResourceNotFoundException(404) — the wire recordings show 400 with ResourceType "WORKSPACE_INSTANCE", which is what Causes 3 and 4 cover and which that issue does not. Open stub issues exist for ListInstanceTypes and ListRegions, but both operations do answer validation here (400 bodies), so Causes 1 and 2 are not blocked by them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 19 steps, 11 mismatched, state_reset_failures 0).

Cause 1 — fixture workspaces-instances-catalog-discovery step task#3, ListInstanceTypes: expected status 400, got 400; $.message value_mismatch — expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 600", got "1 validation error detected: Value '601' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 600".

Cause 2 — fixture workspaces-instances-catalog-discovery step task#4, ListRegions: expected status 400, got 400; $.message value_mismatch — expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25", got "1 validation error detected: Value '100' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25".

Cause 3 — fixture workspaces-instances-get-workspace-instance-not-found step task#1, GetWorkspaceInstance: status expected 400, got 404; $.ResourceType value_mismatch (expected 'WORKSPACE_INSTANCE', got 'workspace-instance'); $.Message value_mismatch (expected 'Workspace instance is not found', got 'WorkSpace Instance wsinst-00000000notreal does not exist') — message cited as an example only.

Cause 4 — fixture workspaces-instances-delete-workspace-instance-not-found step task#1, DeleteWorkspaceInstance: status expected 400, got 404; $.ResourceType value_mismatch (expected 'WORKSPACE_INSTANCE', got 'workspace-instance'); $.Message value_mismatch (expected 'Workspace instance is not found', got 'WorkSpace Instance wsinst-00000000notreal does not exist') — message cited as an example only.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
2. Cause 1: call ListInstanceTypes with MaxResults above the modeled maximum (e.g. 601) -> 400 ValidationException; inspect the message: it contains "Value '601' at 'maxResults'" instead of "Value at 'maxResults'". Fixture workspaces-instances-catalog-discovery step task#3.
3. Cause 2: call ListRegions with MaxResults above the modeled maximum (e.g. 100) -> 400 ValidationException; same value echo. Fixture workspaces-instances-catalog-discovery step task#4.
4. Cause 3: call GetWorkspaceInstance with a well-formed but non-existent id (e.g. wsinst-00000000notreal) -> emulator answers HTTP 404 with ResourceType "workspace-instance"; the recording answers HTTP 400 with ResourceType "WORKSPACE_INSTANCE". Fixture workspaces-instances-get-workspace-instance-not-found step task#1.
5. Cause 4: call DeleteWorkspaceInstance with the same non-existent id -> same 404 / "workspace-instance" divergence. Fixture workspaces-instances-delete-workspace-instance-not-found step task#1.
6. Re-replay all four steps (issue_trace/issue_step plus the three fixture#step tokens in trace_ids) to confirm every cause is closed.
