Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.create-policy-from-template-resolves-placeholders verifiedpermissions.delete-policy-template-conflict-with-linked-policy verifiedpermissions.update-policy-immutable-principal-mutation-rejected verifiedpermissions.put-schema-strict-mode-validates-policy-actions
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|CreatePolicy|NotImplemented
Issue-Key: verifiedpermissions.update-policy-immutable-principal-mutation-rejected|run#8|AssertionError|assert err["Error"]["Code"] == "ValidationException", (
Issue-Key: verifiedpermissions.put-schema-strict-mode-validates-policy-actions|run#12|AssertionError|assert err.get("fieldList") == expected_fields, f"unexpected fieldList={err.get('fieldList')!r}"
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# VerifiedPermissions control plane: template-linked CreatePolicy, UpdatePolicy, and STRICT-mode ValidationException fieldList all diverge

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Three distinct root causes in the Verified Permissions control plane, all in service-com-amazonaws-verifiedpermissions-verifiedpermissions. Each must be closed before this issue is submitted.

Cause 1 of 3: CreatePolicy refuses a `templateLinked` policy definition with 501 NotImplemented.
AWS: Smithy — CreatePolicy.definition targets PolicyDefinition, a union whose arms are `static` and `templateLinked` (TemplateLinkedPolicyDefinition: policyTemplateId + principal/resource for the ?principal/?resource slots). CreatePolicy models errors ConflictException(409), ResourceNotFoundException(404), ServiceQuotaExceededException(402), AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400) — `NotImplemented` is not among them, and 501 is not a modeled status for this operation. A valid templateLinked create returns 200 with policyType TEMPLATE_LINKED and the principal/resource bound at create time.
Emulator: `An error occurred (NotImplemented) when calling the CreatePolicy operation: operation CreatePolicy with a templateLinked definition is scaffolded but not implemented in this emulator`. The `static` arm works; only the templateLinked arm is a stub, so the operation is not flagged as an unimplemented operation anywhere.
Why: template-linked policies are the whole point of policy templates — a client that instantiates a template gets an unmodeled 501 instead of a policy, so placeholder resolution, template-linked authorization decisions, and DeletePolicyTemplate's cascade onto linked policies are all unreachable.
Fix: the templateLinked branch of the CreatePolicy handler (and the template-link state it must record so GetPolicy/ListPolicies/DeletePolicyTemplate see it).
Traces: verifiedpermissions.create-policy-from-template-resolves-placeholders, verifiedpermissions.delete-policy-template-conflict-with-linked-policy.

Cause 2 of 3: UpdatePolicy answers 501 NotImplemented for every request.
AWS: Smithy — UpdatePolicy is a modeled operation (input policyStoreId/policyId/definition/name; output policyStoreId/policyId/policyType/principal/resource/actions/createdDate/lastUpdatedDate/effect) whose errors are ConflictException(409), ResourceNotFoundException(404), ServiceQuotaExceededException(402), AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400). The modeled docs state which elements of a static policy may change (the action, and when/unless conditions) and which may not (static→templateLinked, the effect, the principal, the resource). A replacement statement that changes the principal in the policy scope is therefore refused with ValidationException (400); the recorded response carries message "Invalid input" and a fieldList entry {path: "/definition/static/statement", message: "You can't change the principal referenced in the scope of an existing policy."} — the rule is the model's immutability rule, that text is the recorded example — and the stored policy keeps its original principal. An update that changes only the action succeeds with 200.
Emulator: every UpdatePolicy call returns error code `NotImplemented`. The handler is the untouched scaffold: services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/update_policy.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdatePolicy"))` (line 214).
Why: a client cannot edit a policy at all, and — worse for an authorization emulator — it gets no signal that principal/resource/effect mutation is forbidden, so code that silently relies on UpdatePolicy refusing a scope change is never exercised.
Fix: implement the UpdatePolicy handler: resolve the store and policy, reject a definition that changes principal, resource, effect or policy kind with the modeled ValidationException (400, one fieldList entry naming /definition/static/statement), apply an action/condition-only change, and return the modeled output with lastUpdatedDate advanced.
Traces: verifiedpermissions.update-policy-immutable-principal-mutation-rejected.

Cause 3 of 3: CreatePolicy's STRICT-mode schema-validation ValidationException reports the wrong fieldList — wrong `path` and only one entry where AWS reports two.
AWS: Smithy — ValidationException.fieldList is a list of ValidationExceptionField{path (required), message (required)}, and the ValidationException doc enumerates the distinct validation reasons, including **UnrecognizedActionId** ("the policy includes an action id that isn't found in the schema") and **InvalidActionApplication** ("the policy includes an action that, according to the schema, doesn't support the specified principal and resource"). A static policy naming an undeclared action in a STRICT store is refused 400 ValidationException with message "Invalid input" and TWO fieldList entries, both with path `/definition` — one per reason — e.g. 'for policy `policy0`, unrecognized action `PhotoApp::Action::"deletePhoto"`' and 'for policy `policy0`, unable to find an applicable action given the policy scope constraints'. The requirement is the entry count (one per validation reason raised) and the path value; the message strings are the recorded examples.
Emulator: the status, code and top-level message are right, but fieldList is a single entry at path `/definition/static/statement` carrying a concatenated message: [{'path': '/definition/static/statement', 'message': 'the policy does not validate against the schema in this policy store: for policy `policy0`, unrecognized action `PhotoApp::Action::"deletePhoto"`'}].
Why: clients and consoles key remediation off `path` to point at the offending request member and iterate fieldList to surface every validation reason; a single entry at a deeper, non-AWS path means a client either fails to match the field or reports only one of the two reasons the schema check actually raised.
Fix: the CreatePolicy/validation path that builds the policy-schema-validation ValidationException — emit one ValidationExceptionField per Cedar validation reason and anchor `path` at `/definition` for schema-validation failures (as distinct from request-shape failures).
Traces: verifiedpermissions.put-schema-strict-mode-validates-policy-actions.

## Evidence
Cause 1 — expected: CreatePolicy with definition={'templateLinked': {...}} returns 200 with policyType TEMPLATE_LINKED. Actual: ClientError (NotImplemented) "operation CreatePolicy with a templateLinked definition is scaffolded but not implemented in this emulator".

Cause 2 — expected: UpdatePolicy with a statement that changes the scope principal -> 400 ValidationException, Message "Invalid input", fieldList [{path: /definition/static/statement, message: "You can't change the principal referenced in the scope of an existing policy."}]; an action-only update -> 200. Actual: AssertionError: expected ValidationException, got 'NotImplemented' (handler is the scaffold stub, update_policy.rs:214).

Cause 3 — expected fieldList: [{'path': '/definition', 'message': 'for policy `policy0`, unrecognized action `PhotoApp::Action::"deletePhoto"`'}, {'path': '/definition', 'message': 'for policy `policy0`, unable to find an applicable action given the policy scope constraints'}]. Actual fieldList: [{'path': '/definition/static/statement', 'message': 'the policy does not validate against the schema in this policy store: for policy `policy0`, unrecognized action `PhotoApp::Action::"deletePhoto"`'}] (status 400, code ValidationException and Message "Invalid input" all matched).

## Steps to Reproduce
Cause 1: CreatePolicyStore(validationSettings={mode: OFF}) -> CreatePolicyTemplate with a statement using ?principal/?resource -> CreatePolicy(policyStoreId, definition={'templateLinked': {'policyTemplateId': tid, 'principal': {...}, 'resource': {...}}}). Observe NotImplemented. Same call blocks verifiedpermissions.delete-policy-template-conflict-with-linked-policy.

Cause 2: CreatePolicyStore -> CreatePolicy(definition={'static': {'statement': 'permit(principal == User::"alice", action == Action::"viewPhoto", resource == Photo::"vacation.jpg");'}}) -> UpdatePolicy(same policyId, definition={'static': {'statement': ... principal == User::"bob" ...}}). Observe NotImplemented instead of ValidationException; then UpdatePolicy with only the action changed and observe it also fails instead of succeeding.

Cause 3: CreatePolicyStore(mode OFF) -> PutSchema with a cedarJson declaring PhotoApp::Action::"viewPhoto" only -> UpdatePolicyStore(validationSettings={mode: STRICT}) -> CreatePolicy with a static statement naming action PhotoApp::Action::"deletePhoto". Inspect the ValidationException's fieldList.
