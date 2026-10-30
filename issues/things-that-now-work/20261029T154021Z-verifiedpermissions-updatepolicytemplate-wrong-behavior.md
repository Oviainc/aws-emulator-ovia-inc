Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# A template whose scope names the emulator's own slot sentinel entity defeats the principal/resource immutability check, leaving linked policies unresolvable

_Filed: 2026-10-29T15:40:21Z_
_Operation: UpdatePolicyTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePolicyTemplate` holds a replacement to the elements its model makes immutable by comparing the SCOPE Cedar reports for the stored template with the scope it reports for the replacement (`crate::state::cedar::template_scope`). `template_scope` obtains a scope by LINKING the template against a fixed sentinel entity — `EmulatorInternal::TemplateSlot::"unbound"` — in every slot the template uses, so that "a position holding a placeholder ... reports the sentinel ... and one naming an entity reports that entity".

The sentinel is an ordinary Cedar entity reference that a caller may write in a `statement`: nothing in the crate refuses a template whose scope names `EmulatorInternal::TemplateSlot::"unbound"` concretely. For such a template the two cases the comparison relies on telling apart become indistinguishable, so a replacement may turn a CONCRETE scope position into a `?principal` / `?resource` placeholder (or the reverse) and the immutability check reports no change.

The consequences are observable: the store then holds `TemplateLink`s whose bindings no longer match the template's slots. Every later authorization request against the store hits the link failure in `load_policies`, which pushes an evaluation error and drops the policy from the policy set, so a policy that previously granted access silently stops taking part in any decision — while `GetPolicy`/`ListPolicies` keep reporting its stale `principal`/`resource` and `CreatePolicy` now demands a binding the stored links do not carry. The handler's own comment asserts the sentinel "is private to this module and never reaches a response", which is true of the RESPONSE but not of the request: a caller can put it in a statement.

## Evidence

The sentinel and the claim that it can only ever come from this module — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state/cedar.rs:592-601:
```
/// It stands for "whatever the policy that instantiates this template will bind
/// here", so two templates that both leave a scope position open compare EQUAL
/// in that position while one that names a concrete entity there does not. It is
/// private to this module and never reaches a response: `template_scope` reports
/// the scope, not the entity.
const SLOT_SENTINEL_TYPE: &str = "EmulatorInternal::TemplateSlot";
const SLOT_SENTINEL_ID: &str = "unbound";
```
and the rule it is supposed to make decidable — src/state/cedar.rs:609-618:
```
/// So the template is linked against
/// [`SLOT_SENTINEL_TYPE`] in each slot it uses and the scope is read off that
/// link: a position holding a placeholder then reports the sentinel, which is
/// equal across templates, and one naming an entity reports that entity.
```
A statement naming that entity concretely is read by the same code path (src/state/cedar.rs:619-632), and nothing anywhere in the crate rejects it: `grep SLOT_SENTINEL` matches only src/state/cedar.rs.

The check that is thereby defeated — src/ops/update_policy_template.rs:241-256:
```
        // The replacement must be Cedar TEMPLATE text (which also bounds its
        // size), and its scope is what the immutability rule is stated over.
        let replacement = crate::state::cedar::template_scope(&input.statement).map_err(invalid)?;
        ...
        let immutable = immutable_changes(&stored, &replacement);
        if !immutable.is_empty() {
            return Err(refuse(immutable));
        }
```
and src/ops/update_policy_template.rs:392-398, which compares the two scopes member by member:
```
    if stored.principal != replacement.principal {
        changes.push(crate::ops::InvalidInput::new(
            crate::ops::paths::TEMPLATE_STATEMENT,
            "You can't change the principal referenced in the scope of an existing policy \
             template.",
        ));
    }
```
The model sentence the check implements (`smithy_query(query="shape", shape="UpdatePolicyTemplateInput")`, member `statement`): "You <b>can't</b> change the following elements: The effect (permit or forbid) of the policy template. The <code>principal</code> referenced by the policy template. The <code>resource</code> referenced by the policy template."

What the surviving links then do on every authorization call — src/state/cedar.rs:864-881:
```
                let values = match slot_values(link) { ... };
                let link_id = cedar_policy::PolicyId::new(policy_id);
                if let Err(e) = set.link(template_pid, link_id, values) {
                    errors.push(format!(
                        "policy {policy_id} could not be linked to policy template \
                         {template_id}: {e}"
                    ));
                }
```
(the policy is never added to the set, so it determines nothing), while the same link keeps being reported by src/ops/get_policy.rs:216-224.

Test coverage: tests/all/policy_templates.rs and tests/all/template_linked_policies.rs exercise the immutability refusals and the link lifecycle; no case sends a statement whose scope names `EmulatorInternal::TemplateSlot::"unbound"`, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode OFF; note policyStoreId.
2. CreatePolicyTemplate with statement `permit(principal == EmulatorInternal::TemplateSlot::"unbound", action, resource == ?resource);` — it parses as a template (it uses the ?resource slot) and is stored.
3. CreatePolicy with a templateLinked definition binding only `resource` (the template's one slot) — succeeds; GetPolicy reports the policy with principal EmulatorInternal::TemplateSlot::"unbound".
4. UpdatePolicyTemplate on the same template with statement `permit(principal == ?principal, action, resource == ?resource);`.
5. Observed: HTTP 200 — the principal position changed from a concrete entity to a placeholder, which the operation documents as unchangeable (compare any other principal change, which answers 400 ValidationException from immutable_changes).
6. IsAuthorized against the store: the linked policy no longer takes part in the decision and the response's `errors` carries "policy <id> could not be linked to policy template <id>: ...", while GetPolicy still reports the policy and its old principal.
