Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.is-authorized-context-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|IsAuthorized|ValidationException
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# IsAuthorized returns 400 ValidationException for a context value whose type mismatches the schema instead of a DENY with an evaluation error

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IsAuthorized

## Description
Cause 1 of 1: a Cedar evaluation-time type error is surfaced as a request-level refusal instead of as a failed-closed decision.

AWS: Smithy — IsAuthorizedOutput requires three members: `decision` (ALLOW|DENY), `determiningPolicies`, and `errors` (EvaluationErrorList of EvaluationErrorItem{errorDescription}), documented as "Errors that occurred while making an authorization decision, for example, a policy references an Entity or entity Attribute that does not exist in the slice". The existence of a REQUIRED `errors` member on the 200 response is the model's statement that evaluation failures are reported in-band, not as a 4xx. So when a schema declares an action's context attribute as Long and a policy's `when` clause applies a Long-only operator (`>`) to it, a request supplying that attribute as a boolean is still EVALUATED: the response is 200 with decision DENY (fails closed), a non-empty `errors` list describing the type error, and `determiningPolicies` empty — the erroring policy must not determine the decision. The well-typed control request (count as a long) returns 200 ALLOW, errors == [], determiningPolicies == [{policyId}]. ValidationException (400) is modeled on IsAuthorized, but for a request that does not satisfy its input contract, not for a policy that errors during evaluation.

Emulator: the first IsAuthorized call is rejected outright — `botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the IsAuthorized operation: Invalid input`. In services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/is_authorized.rs the result of `crate::state::cedar::authorize` is mapped with `Refusal::InvalidInput(field) => invalid(field)` (lines 202-214), i.e. a schema/type refusal raised while evaluating the request against the store's schema is turned into the modeled 400 ValidationException rather than into a DENY whose `errors` list carries the description. No decision is ever returned, so the check never reaches the positive control.

Why: an authorization client sees an exception where AWS gives it a decision. Callers that treat "IsAuthorized raised" as a bug/retry path break, and — the dangerous direction — any caller that wraps IsAuthorized and falls back on an exception can convert what AWS makes a hard DENY into an allow. It also hides the whole fail-closed-with-errors contract: `errors` and the "a policy that errors does not determine the decision" rule are never exercised.

Fix: the evaluation-refusal mapping in the IsAuthorized handler (and the shared Cedar layer that classifies refusals): distinguish a malformed REQUEST (missing triple member, unparseable contextMap/entities → ValidationException 400, which is already correct) from a per-policy EVALUATION error against the store schema, which must be collected into IsAuthorizedOutput.errors with decision DENY and determiningPolicies empty. The same classification governs IsAuthorizedWithToken/BatchIsAuthorized, which share the refusal type.

Traces: verifiedpermissions.is-authorized-context-type-mismatch-rejected.

## Evidence
Expected (AWS): HTTP 200, {"decision": "DENY", "determiningPolicies": [], "errors": [<non-empty, describing the Long/boolean operand mismatch>]}; then for context={"contextMap": {"count": {"long": 5}}} HTTP 200 {"decision": "ALLOW", "errors": [], "determiningPolicies": [{"policyId": <id>}]}.
Actual (emulator): HTTP 400, code ValidationException, message "Invalid input" on the first IsAuthorized call — no decision returned at all.
Code path: is_authorized.rs lines 202-214 map `crate::state::cedar::Refusal::InvalidInput` to the modeled ValidationException.

## Steps to Reproduce
1. CreatePolicyStore(validationSettings={"mode": "OFF"}, tags={"sim2real": "true"}).
2. PutSchema with cedarJson declaring namespace PhotoApp, entityTypes User/Photo, and action viewPhoto whose appliesTo.context is {"type": "Record", "attributes": {"count": {"type": "Long", "required": true}}}.
3. CreatePolicy with static statement: permit(principal == PhotoApp::User::"alice", action == PhotoApp::Action::"viewPhoto", resource == PhotoApp::Photo::"vacation.jpg") when { context.count > 0 };
4. IsAuthorized(policyStoreId, principal PhotoApp::User/alice, action PhotoApp::Action/viewPhoto, resource PhotoApp::Photo/vacation.jpg, context={"contextMap": {"count": {"boolean": true}}}). The emulator raises ValidationException; AWS returns 200 DENY with a populated errors list and empty determiningPolicies.
5. Repeat step 4 with context={"contextMap": {"count": {"long": 5}}} — must be 200 ALLOW with errors == [].
