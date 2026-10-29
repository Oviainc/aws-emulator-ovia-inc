Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f

# A replacement template that changes the entity TYPE in a `principal is T` / `resource is T` scope is accepted, and every linked policy silently follows the new type

_Filed: 2026-10-29T09:35:30Z_
_Operation: UpdatePolicyTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePolicyTemplate` holds the replacement to the elements `UpdatePolicyTemplateInput$statement` makes immutable by comparing two `StaticScope`s produced by `crate::state::cedar::template_scope`. That scope is the same lossy projection `UpdatePolicy` uses: `scope_of` maps `PrincipalConstraint::Is(_)` to `None` and `IsIn(_, uid)` to the uid alone, discarding the entity type.

So a replacement that changes `principal is PhotoApp::User` to `principal is PhotoApp::Admin` (or `resource is PhotoApp::Photo` to `resource is PhotoApp::Album`, or the type inside an `is T in E` scope) passes `immutable_changes` with an empty list, the template's `statement` is overwritten, and 200 is returned — even though the operation documents the principal and the resource the template references as unchangeable.

The blast radius is wider than on `UpdatePolicy`: `load_policies` re-reads the template's text on every evaluation, so every policy linked to the template immediately decides against the new entity type. The handler's own comment asserts this cannot happen, which is the second half of the defect: it states that "its effect, its principal and its resource cannot differ from the stored template's (the check above refused a replacement that changed one)" and therefore only copies `replacement.actions` onto the linked policies — a claim the `is T` case breaks.

## Evidence

The modelled rule, `UpdatePolicyTemplateInput$statement`: "You can change only the following elements of the policy body: The `action` referenced by the policy template. Any conditional clauses, such as `when` or `unless` clauses. You **can't** change the following elements: The effect (`permit` or `forbid`) of the policy template. The `principal` referenced by the policy template. The `resource` referenced by the policy template."

The comparison, `src/ops/update_policy_template.rs:381-405`:

    fn immutable_changes(
        stored: &crate::state::cedar::StaticScope,
        replacement: &crate::state::cedar::StaticScope,
    ) -> Vec<crate::ops::InvalidInput> {
        ...
        if stored.principal != replacement.principal {
        ...
        if stored.resource != replacement.resource {

The projection both scopes come from, `src/state/cedar.rs:345-356`:

        let principal = match policy.principal_constraint() {
            cedar_policy::PrincipalConstraint::Any | cedar_policy::PrincipalConstraint::Is(_) => None,
            cedar_policy::PrincipalConstraint::Eq(uid)
            | cedar_policy::PrincipalConstraint::In(uid)
            | cedar_policy::PrincipalConstraint::IsIn(_, uid) => Some(scope_entity(&uid)),
        };

The handler comment the code breaks, `src/ops/update_policy_template.rs:328-336`:

        // Every policy linked to this template follows it, and the ONLY part of
        // such a policy's reported scope a replacement can change is the actions
        // the template's scope names: its effect, its principal and its resource
        // cannot differ from the stored template's (the check above refused a
        // replacement that changed one) ...

followed by the write at `src/ops/update_policy_template.rs:316` (`template.statement = input.statement.clone();`) and the loop at 341-345 that updates only `policy.actions`.

The linked policies re-read the replaced text on every evaluation — `src/state/cedar.rs:777-780`:

            (None, Some(link)) => {
                match store.policy_templates.get(&link.policy_template_id) {
                    Some(template) => template.statement.as_str(),

Test coverage: `tests/all/policy_templates.rs` and `tests/all/update_and_delete_lifecycle.rs` exercise template updates and the immutability refusals for scopes that name entities or placeholders; `tests/all/policy_templates.rs:530-561` uses `principal is PhotoApp::User` / `resource is PhotoApp::Photo` only in CREATE rows, never as the stored side of an update, so nothing pins the behaviour described here.

## Steps to Reproduce

1. CreatePolicyStore(validationSettings={mode: OFF}).
2. CreatePolicyTemplate(statement='permit(principal is PhotoApp::User, action == PhotoApp::Action::"viewPhoto", resource == ?resource);') and note the policyTemplateId.
3. CreatePolicy(definition={templateLinked: {policyTemplateId, resource: {entityType: "PhotoApp::Photo", entityId: "vacation.jpg"}}}).
4. UpdatePolicyTemplate(same policyTemplateId, statement='permit(principal is PhotoApp::Admin, action == PhotoApp::Action::"viewPhoto", resource == ?resource);').
5. Observe HTTP 200 instead of a ValidationException naming /template/statement for the changed principal; IsAuthorized for PhotoApp::User::"alice" now DENYs where it previously ALLOWed, because the linked policy followed the new entity type.
