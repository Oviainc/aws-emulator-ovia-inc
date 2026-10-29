Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f

# A replacement that changes the entity TYPE in a `principal is T` / `resource is T` scope is accepted, although the operation documents those elements as unchangeable

_Filed: 2026-10-29T09:35:30Z_
_Operation: UpdatePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePolicy` holds a replacement statement to the elements its own model documentation makes immutable by comparing the stored policy's scope against the replacement's parsed `StaticScope`. But `StaticScope.principal`/`StaticScope.resource` only ever carry an ENTITY: `cedar.rs`'s `scope_of` maps `PrincipalConstraint::Is(_)` to `None` and `PrincipalConstraint::IsIn(_, uid)` to the uid alone, discarding the entity type in both cases.

So two scopes that reference DIFFERENT principals compare equal and the update is accepted:

* `permit(principal is PhotoApp::User, ...)` -> `permit(principal is PhotoApp::Admin, ...)` — both project to `principal: None`;
* `permit(principal is PhotoApp::User in PhotoApp::Group::"eng", ...)` -> `permit(principal is PhotoApp::Admin in PhotoApp::Group::"eng", ...)` — both project to `Some(Group::"eng")`;
* the same two shapes in the `resource` position.

The handler then writes `policy.statement = Some(def.statement)` and answers 200, so the stored policy now references a principal the operation documents as unchangeable, and every later authorization decision uses the new type. The lossy projection is correct for what it was built for — `GetPolicyOutput$principal` is documented as "isn't present when principal isn't specified in the policy content" — but reusing it as the immutability comparison silently drops the only thing that distinguishes `is User` from `is Admin`.

## Evidence

The modelled rule, `UpdatePolicyInput$definition`: "You can change only the following elements from the policy definition: The `action` referenced by the policy. Any conditional clauses, such as `when` or `unless` clauses. You **can't** change the following elements: Changing from `static` to `templateLinked`. Changing the effect of the policy from `permit` or `forbid`. The `principal` referenced by the policy. The `resource` referenced by the policy."

The comparison, `src/ops/update_policy.rs:435-457`:

    fn immutable_changes(
        stored: &crate::state::Policy,
        replacement: &crate::state::cedar::StaticScope,
    ) -> Vec<crate::ops::InvalidInput> {
        ...
        if stored.principal != replacement.principal {
        ...
        if stored.resource != replacement.resource {

The projection it compares, `src/state/cedar.rs:345-356`:

        let principal = match policy.principal_constraint() {
            cedar_policy::PrincipalConstraint::Any | cedar_policy::PrincipalConstraint::Is(_) => None,
            cedar_policy::PrincipalConstraint::Eq(uid)
            | cedar_policy::PrincipalConstraint::In(uid)
            | cedar_policy::PrincipalConstraint::IsIn(_, uid) => Some(scope_entity(&uid)),
        };

and its own doc, `src/state/cedar.rs:287-292`:

        /// Present only when the scope names a principal entity (`principal ==`,
        /// `principal in`, `principal is T in`); absent for `principal` and
        /// `principal is T`, which name no entity.

The write that follows a passing check, `src/ops/update_policy.rs:366-367`:

        if let Some((def, scope)) = replacement {
            policy.statement = Some(def.statement);

Test coverage: `tests/all/policy_update.rs:380-430` (`an_update_that_changes_an_immutable_element_is_refused`) enumerates the immutable-element rows — "a different principal entity", "a principal named by type only, where it was an entity", "the same principal entity reached with in instead of ==", "a different resource entity", "an unconstrained resource, where it was an entity", "the effect" — every one of which stores a scope that names an ENTITY. No row stores a `principal is T` / `resource is T` scope, so nothing pins the behaviour described here.

## Steps to Reproduce

1. CreatePolicyStore(validationSettings={mode: OFF}).
2. CreatePolicy(definition={static: {statement: 'permit(principal is PhotoApp::User, action == PhotoApp::Action::"viewPhoto", resource == PhotoApp::Photo::"vacation.jpg");'}}).
3. UpdatePolicy(same policyId, definition={static: {statement: 'permit(principal is PhotoApp::Admin, action == PhotoApp::Action::"viewPhoto", resource == PhotoApp::Photo::"vacation.jpg");'}}).
4. Observe HTTP 200 instead of the ValidationException the operation documents for a change of the principal the policy references; GetPolicy reports the new statement.
5. Repeat with stored 'permit(principal is PhotoApp::User in PhotoApp::Group::"eng", ...)' replaced by 'permit(principal is PhotoApp::Admin in PhotoApp::Group::"eng", ...)' and with the same two shapes in the resource position — all are accepted.
