Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 909749a7cea0ac224aeb09b9b62bd72ae5efa8b6

# CreatePolicyTemplate stores an empty-string name as a name, while UpdatePolicyTemplate normalises the same value to "no name"

_Filed: 2026-10-28T18:43:29Z_
_Operation: CreatePolicyTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `909749a7cea0ac224aeb09b9b62bd72ae5efa8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PolicyTemplateName` is modelled `len 0..150`, so `name: ""` reaches the handler. `CreatePolicyTemplate` persists the member verbatim, so the template is stored with `Some("")`, while `UpdatePolicyTemplate` — for the same member of the same resource, quoting the member's documented rule — maps `""` to no name. Consequently `GetPolicyTemplate` reports `"name": ""` for a created template but no `name` for one that reached the same state via an update with the identical value, a second `CreatePolicyTemplate` carrying `name: ""` in that store is refused with `ConflictException` (the uniqueness check matches `Some("")`), and `PolicyStore::resolve_policy_template_id("name/")` resolves to it.

## Evidence

Create path — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_template.rs:342-347:
```
        let template = crate::state::PolicyTemplate {
            policy_template_id: policy_template_id.clone(),
            policy_store_id: store_id.clone(),
            name: input.name.clone(),
```
Sibling update path — src/ops/update_policy_template.rs:327-335:
```
        // Only `name` carries a documented "unchanged unless included" rule ("If
        // you don't include the name in an update request, the existing name is
        // unchanged. To remove a name, set it to an empty string"), so
        ...
        if let Some(name) = input.name.clone() {
            template.name = (!name.is_empty()).then_some(name);
        }
```
The stored `Some("")` then collides on create — src/ops/create_policy_template.rs:297-311:
```
        if let Some(name) = input.name.as_deref() {
            if let Some(clash) = store
                .policy_templates
                .values()
                .find(|template| template.name.as_deref() == Some(name))
```
Test coverage: tests/all/update_and_delete_lifecycle.rs:1156-1162 pins the update rule (`("an empty name removes it", Some(""), 200, None)`); no test covers create with an empty name.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode = OFF.
2. CreatePolicyTemplate with a valid Cedar template statement and "name": "" — 200.
3. GetPolicyTemplate for the returned policyTemplateId: the response carries "name": "".
4. CreatePolicyTemplate again in the same store with "name": "" — 409 ConflictException, although neither template carries a usable name.
