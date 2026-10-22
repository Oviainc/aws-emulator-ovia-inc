Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.list-regions-malformed-next-token-validation workspaces-instances.get-workspace-instance-malformed-id-validation workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances.list-regions-malformed-next-token-validation|run#4|AssertionError|assert message == (
Issue-Key: workspaces-instances.get-workspace-instance-malformed-id-validation|run#5|AssertionError|assert message == (
Issue-Key: workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation|run#4|AssertionError|assert message == (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# ValidationException message construction diverges from AWS for workspaces-instances (echoes rejected value, wrong list-member path, anchored pattern)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkspaceInstance

## Description
The WorkSpaces Instances emulator returns the right error code (ValidationException) and status (400) for constraint violations, but the message it builds does not follow the construction rule the service uses. The message here is a requirement rather than an example because it is built from the request: it names the offending member path and the constraint that member violated. Three distinct defects in that one formatter, all in the ValidationException message builder shared by the operations' input validation.

Cause 1 of 3 — the rejected value is echoed into the message.
AWS: validation rejections are reported as "1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: <constraint text>" — the member path and the constraint are named, the submitted value is NOT reproduced. Smithy: ListRegions.MaxResults targets MaxResults @range min 1 max 25; GetWorkspaceInstance.WorkspaceInstanceId @pattern "^wsinst-[0-9a-zA-Z]{8,63}$", @length 15..70; both operations model ValidationException (httpError 400). The omission of the value itself is recorded behavior only (three independent recordings for three different members all omit it).
Emulator: inserts the value between "Value" and "at" — e.g. "1 validation error detected: Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25" and "Value 'not-a-valid-workspace-instance-x' at 'workspaceInstanceId' failed to satisfy constraint: ...".
Why: clients and test suites that assert on or diff the validation text see a different message shape for every request, and the emulator reproduces caller-supplied input in an error body where the service does not (NextToken is modeled @sensitive on this service, so echoing input values in validation text is the wrong default here).
Fix: the shared ValidationException message formatter used by the operations' input validation for this service crate — drop the value from the rendered message.
Traces: workspaces-instances.list-regions-malformed-next-token-validation, workspaces-instances.get-workspace-instance-malformed-id-validation, workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation.

Cause 2 of 3 — constrained list members report the wrong path and omit the nested-constraint wrapper.
AWS: when a member of a constrained list fails, the failure is reported against the list member itself and the element constraint is nested: "1 validation error detected: Value at 'provisionStates' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [...]]". Smithy: ListWorkspaceInstances.ProvisionStates is a list whose member targets ProvisionStateEnum (values ALLOCATING, ALLOCATED, DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING); the operation models ValidationException (400). The nesting form is recorded behavior only.
Emulator: reports the element path with an index — "Value 'NOT_A_REAL_STATE' at 'provisionStates.1.member' failed to satisfy constraint: Member must satisfy enum value set: [ALLOCATING, ALLOCATED, DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING]" — with no "Member must satisfy constraint: [...]" wrapper. (The index is also off by one for a single-element list; the enum ordering inside the brackets differs from the recording and is treated here as an example, not a requirement.)
Why: a client that maps the member path in a validation message back to the request field it submitted is pointed at a path ('provisionStates.1.member') the service never emits, and loses the signal that the violation is on a list element rather than on the list.
Fix: list/collection handling in the same ValidationException message formatter (path should be the list member name; element constraints nested in the wrapper form).
Traces: workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation.

Cause 3 of 3 — pattern constraints are rendered with the regex anchors.
AWS: the pattern constraint text is "Member must satisfy regular expression pattern: wsinst-[0-9a-zA-Z]{8,63}" — the leading ^ and trailing $ of the modeled pattern are not rendered. Smithy: WorkspaceInstanceId @pattern "^wsinst-[0-9a-zA-Z]{8,63}$"; the stripping of the anchors is recorded behavior only.
Emulator: renders the modeled pattern verbatim, "Member must satisfy regular expression pattern: ^wsinst-[0-9a-zA-Z]{8,63}$".
Why: a client that lifts the advertised pattern out of the validation message and re-applies it locally gets a different expression than the service advertises.
Fix: pattern-constraint rendering in the same ValidationException message formatter.
Traces: workspaces-instances.get-workspace-instance-malformed-id-validation.

Note: the ValidationException/ResourceNotFoundException discriminators in these checks pass — the emulator does classify a malformed id as ValidationException(400) and a well-formed unknown id as ResourceNotFoundException(404). Only the message construction diverges. No open issue covers this formatter (the open workspaces-* issues are for a different service, WorkSpaces, and are stub/bootstrap items).

## Evidence
Check workspaces-instances.list-regions-malformed-next-token-validation — expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25"; emulator returned "1 validation error detected: Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25" (code ValidationException matched).

Check workspaces-instances.get-workspace-instance-malformed-id-validation — expected "1 validation error detected: Value at 'workspaceInstanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: wsinst-[0-9a-zA-Z]{8,63}"; emulator returned "1 validation error detected: Value 'not-a-valid-workspace-instance-x' at 'workspaceInstanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^wsinst-[0-9a-zA-Z]{8,63}$" (code ValidationException matched).

Check workspaces-instances.list-workspace-instances-invalid-provision-state-enum-validation — expected "1 validation error detected: Value at 'provisionStates' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [DEALLOCATED, ALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING, ALLOCATING, DEALLOCATING]]"; emulator returned "1 validation error detected: Value 'NOT_A_REAL_STATE' at 'provisionStates.1.member' failed to satisfy constraint: Member must satisfy enum value set: [ALLOCATING, ALLOCATED, DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING, ERROR_DEALLOCATING]" (code ValidationException matched).

## Steps to Reproduce
Against the emulator with a workspaces-instances client:
1. wsi.list_regions(MaxResults=999) -> ValidationException; inspect Error.Message: it contains "Value '999' at 'maxResults'" instead of "Value at 'maxResults'". (Cause 1)
2. wsi.get_workspace_instance(WorkspaceInstanceId="not-a-valid-workspace-instance-x") -> ValidationException; the message echoes the submitted value and renders the pattern as "^wsinst-[0-9a-zA-Z]{8,63}$" instead of "wsinst-[0-9a-zA-Z]{8,63}". (Causes 1 and 3)
3. wsi.list_workspace_instances(ProvisionStates=["NOT_A_REAL_STATE"]) -> ValidationException; the message reports path 'provisionStates.1.member' and omits the "Member must satisfy constraint: [...]" wrapper around the enum constraint. (Causes 1 and 2)
Re-replay the three checks in trace_ids to confirm.
