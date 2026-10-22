Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-policy-crud-with-schema
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-policy-crud-with-schema
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.actions
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.createdDate
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.effect
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.lastUpdatedDate
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.policyId
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.policyStoreId
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.policyType
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.principal
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|$.resource
Issue-Key: verifiedpermissions|CreatePolicy|2xx->4xx|status
Issue-Key: verifiedpermissions|ListPolicies|2xx->2xx|$.policies[*]
Issue-Key: verifiedpermissions|CreatePolicy|4xx->4xx|$.fieldList
Issue-Key: verifiedpermissions|CreatePolicy|4xx->4xx|$.message
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Verified Permissions CreatePolicy rejects valid static policies in a STRICT store and omits ValidationException fieldList

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
In a policy store whose validation mode is STRICT and whose schema has been uploaded, the emulator refuses to create a policy that AWS accepts, so the policy never exists and never lists; and when it does legitimately reject a policy it emits a ValidationException without the modeled fieldList that tells the client what failed. This bundle has 3 causes.

Cause 1 of 3: CreatePolicy returns 400 where AWS returns 200 for a schema-conformant static policy.
AWS: Smithy CreatePolicy returns 200 with required policyStoreId, policyId, policyType, createdDate, lastUpdatedDate plus principal/resource/actions/effect extracted from the policy statement. ValidationException (httpError 400) is modeled for a policy that does not satisfy the store's schema. In the recording the store has had its schema uploaded and a STRICT-mode CreatePolicy of a permit statement over PhotoApp::User / PhotoApp::Action "view" / PhotoApp::Photo succeeds with policyType STATIC and effect Permit.
Emulator: returns status 400 and no response body members at all — policyStoreId, policyId, policyType, principal, resource, actions, createdDate, lastUpdatedDate and effect are all absent. The refusal text the emulator produces elsewhere in the same fixture ("policy store <id> has validation mode STRICT but contains no schema") indicates the store it validated against has no schema registered. Hypothesis: the schema upload that precedes this step never lands in state — PutSchema is an unimplemented scaffold stub in this build (see open issue 20261022T031055Z-verifiedpermissions-putschema-stub.md, which covers the missing operation itself but not this CreatePolicy divergence), so strict validation has nothing to validate against and fails closed.
Why: a client that follows the documented order (create store, PutSchema, create policies, authorize) cannot create any policy in a STRICT store against the emulator; the whole schema-backed authorization workflow is unreachable, and the failure arrives as a client-side 400 that looks like the caller's policy is invalid.
Fix: CreatePolicy's strict-validation path in the verifiedpermissions crate (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy.rs) must validate against the policy store's stored schema and accept a conformant policy — which requires the store's schema actually being persisted by the schema-put path.
Traces: verifiedpermissions-policy-crud-with-schema#task#3.

Cause 2 of 3: ListPolicies reports an empty policies list for a store that should hold the created policy.
AWS: ListPolicies returns a required policies list; each item carries policyStoreId, policyId, policyType, principal, resource, actions, definition.static, createdDate, lastUpdatedDate and effect. A store with one static policy lists exactly that item.
Emulator: returns 200 with $.policies[0] missing — the list is empty.
Why: downstream of cause 1 the store genuinely holds no policy, so this step confirms the create was lost rather than merely mis-serialized; a client enumerating its policies sees none and may re-create or conclude the store is empty. It is listed as its own cause because ListPolicies must be re-replayed to prove the create was repaired end-to-end — a CreatePolicy that returns 200 but does not persist would still fail here.
Fix: no separate ListPolicies defect is implied if the create persists; verify the policy store's policy collection in the crate's state is populated by CreatePolicy and read back by src/ops/list_policies.rs.
Traces: verifiedpermissions-policy-crud-with-schema#task#5.

Cause 3 of 3: the ValidationException for a policy that fails validation carries no fieldList.
AWS: Smithy ValidationException (httpError 400) has required message and an optional-but-populated fieldList of ValidationExceptionField, each with required path and message — the model documents it as "the list of fields that aren't valid", provided "as a list of fields and a reason for each field that isn't valid". The recording shows fieldList = [{path: "/definition/static/statement", message: "Failed to validate policy body"}] with message "Invalid input". The requirement here is the rule: a policy-validation rejection must name the offending input path in fieldList rather than carry it only in prose.
Emulator: returns 400 with the correct ValidationException status but no fieldList key at all, putting its whole explanation in the message (recorded example of the emulator's text: "policy store <id> has validation mode STRICT but contains no schema, so the static policy cannot be validated").
Why: fieldList is the machine-readable part of this error; SDK users and consoles render it to point at the bad member, so a client that inspects e.response["fieldList"] to highlight /definition/static/statement gets nothing and must fall back to parsing free text.
Fix: the ValidationException construction on CreatePolicy's validation path (src/ops/create_policy.rs and the crate's shared error builders) must populate fieldList with the path of the member that failed.
Traces: verifiedpermissions-policy-crud-with-schema#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator.

Cause 1 — fixture verifiedpermissions-policy-crud-with-schema, step task#3, CreatePolicy:
  status: expected 200, got 400
  $.policyStoreId: missing (expected 'PSJPfe8fgQnwl8FAu3TKk6j5', got None)
  $.policyId: missing; $.policyType: missing (expected 'STATIC'); $.effect: missing (expected 'Permit')
  $.principal: missing (expected {'entityType': 'PhotoApp::User', 'entityId': 'sim2real-probe-verifiedpermissions-alice-d5b8d7ed'})
  $.resource: missing (expected {'entityType': 'PhotoApp::Photo', 'entityId': 'sim2real-probe-verifiedpermissions-vacation-0dc5ab47'})
  $.actions: missing (expected [{'actionType': 'PhotoApp::Action', 'actionId': 'view'}])
  $.createdDate / $.lastUpdatedDate: missing

Cause 2 — fixture verifiedpermissions-policy-crud-with-schema, step task#5, ListPolicies:
  expected status 200, actual status 200
  $.policies[0]: missing (expected the STATIC Permit policy item with policyStoreId/policyId/policyType/principal/resource/actions/definition.static/createdDate/lastUpdatedDate/effect, got None)

Cause 3 — fixture verifiedpermissions-policy-crud-with-schema, step task#8, CreatePolicy:
  expected status 400, actual status 400
  $.fieldList: missing (expected [{'path': '/definition/static/statement', 'message': 'Failed to validate policy body'}], got None)
  $.message: value_mismatch (expected 'Invalid input', got 'policy store PSJPfe8fgQnwl8FAu3TKk6j5 has validation mode STRICT but contains no schema, so the static policy cannot be validated') — quoted as the example that reveals the missing-schema state; the asserted requirement is the presence of fieldList

Model evidence: CreatePolicyOutput requires policyStoreId, policyId, policyType, createdDate, lastUpdatedDate and carries principal/resource/actions/effect; ListPoliciesOutput.policies is required; ValidationException (@httpError 400) members = message (required) + fieldList (list of ValidationExceptionField{path required, message required}), documented as the list of fields that aren't valid. Related open issue (the blocking upstream operation, not this divergence): 20261022T031055Z-verifiedpermissions-putschema-stub.md.

## Steps to Reproduce
1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}); PutSchema(policyStoreId=store, definition={"cedarJson": <PhotoApp schema>}); UpdatePolicyStore(policyStoreId=store, validationSettings={"mode":"STRICT"}).
3. CreatePolicy(policyStoreId=store, definition={"static":{"statement": 'permit(principal == PhotoApp::User::"alice", action == PhotoApp::Action::"view", resource == PhotoApp::Photo::"vacation");'}}) -> HTTP 400 instead of a 200 carrying policyId/policyType STATIC/effect Permit (cause 1).
4. ListPolicies(policyStoreId=store) -> 200 with an empty policies list (cause 2).
5. CreatePolicy with a policy that genuinely fails validation -> 400 ValidationException whose body has no fieldList naming /definition/static/statement (cause 3).
6. Re-replay verifiedpermissions-policy-crud-with-schema steps task#3, task#5 and task#8.
