Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc

# ListPolicies' policyTemplateId filter ignores the `name/<policyTemplateName>` reference form that every other policyTemplateId path in the crate resolves

_Filed: 2026-10-28T23:03:27Z_
_Operation: ListPolicies | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other place in this crate that reads a `policyTemplateId` request member resolves it through `PolicyStore::resolve_policy_template_id`, which accepts BOTH a template id and the `name/<policyTemplateName>` form the crate itself documents as interchangeable with the id (`CreatePolicy`'s templateLinked arm, `GetPolicyTemplate`, `UpdatePolicyTemplate`, `DeletePolicyTemplate`).

`ListPolicies`' filter is the one path that skips that helper: it compares the caller's `filter.policyTemplateId` string verbatim against the stored link's id. A filter naming the template by `name/<name>` therefore matches nothing and the operation answers an empty `policies` list with HTTP 200 — a silent wrong answer, not a refusal — while the very same string handed to `GetPolicyTemplate` or `DeletePolicyTemplate` in the same store resolves to that template.

`PolicyFilter$policyTemplateId` targets the same `PolicyTemplateId` shape as those members, so the reference form a caller may write is the same; only this handler reads it differently.

## Evidence

The helper the crate already has, and its own statement of the rule — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:1011-1028:

    /// Resolve a `policyTemplateId` request member: either a template id, or
    /// `name/<policyTemplateName>` naming one, which `GetPolicyTemplate`
    /// documents as interchangeable with the id ("You can use the policy
    /// template name in place of the policy template ID. When using a name,
    /// prefix it with name/").
    pub fn resolve_policy_template_id(&self, reference: &str) -> Option<String> {
        match reference.strip_prefix("name/") { ... }

A sibling path that calls it — src/ops/create_policy.rs:358-360:

    let template_id = store
        .resolve_policy_template_id(&def.policy_template_id)
        .ok_or_else(template_not_found)?;

and src/ops/get_policy_template.rs:163-167:

            .and_then(|store| {
                store
                    .resolve_policy_template_id(&input.policy_template_id)

The path that does NOT — src/ops/list_policies.rs:232-238:

    if let Some(policy_template_id) = filter.policy_template_id.as_deref() {
        // Only a template-linked policy was instantiated from a template, so
        // this member selects the policies that follow exactly that template.
        if policy.policy_template_id() != Some(policy_template_id) {
            return false;
        }
    }

`policy.policy_template_id()` returns the stored template ID (src/state.rs:757-762), so a `name/...` reference can never equal it.

Model side (smithy_query shape=PolicyFilter): member `policyTemplateId` targets `PolicyTemplateId` (length 1..200, pattern ^[a-zA-Z0-9-/_]*$), documented "Filters the output to only template-linked policies that were instantiated from the specified policy template." — the same target shape `GetPolicyTemplateInput$policyTemplateId` uses.

Test coverage: tests/all/template_linked_policies.rs:321-345 exercises the filter with the template ID only (`"filter": { "policyTemplateId": template_id }`); no test pins the `name/` form for this filter, so nothing in the crate justifies the divergence. By contrast tests/all/policy_templates.rs:148-153 and tests/all/delete_policy_template.rs:264-281 pin the `name/` form for the reader and the delete.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings {mode: OFF}; note policyStoreId.
2. CreatePolicyTemplate in that store with name "my-template" and a statement containing `?principal`; note policyTemplateId.
3. CreatePolicy with definition {templateLinked: {policyTemplateId: "name/my-template", principal: {...}}} — succeeds, proving the name form resolves on the create path.
4. GetPolicyTemplate with policyTemplateId "name/my-template" — succeeds, proving the name form resolves on the read path.
5. ListPolicies with filter {"policyTemplateId": "name/my-template"} — returns an empty `policies` list.
6. ListPolicies with filter {"policyTemplateId": "<the PT… id>"} — returns the template-linked policy, showing the filter only understands the id form.
