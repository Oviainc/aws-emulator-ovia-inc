Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 909749a7cea0ac224aeb09b9b62bd72ae5efa8b6

# The "STRICT store holds no schema" refusal branch is implemented four times, once per policy/template write handler

_Filed: 2026-10-28T18:43:29Z_
_Operation: CreatePolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `909749a7cea0ac224aeb09b9b62bd72ae5efa8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "if the store's validation mode is STRICT, resolve the store's schema, and refuse with a ValidationException naming /policyStoreId when it holds none" is branched independently in all four write handlers that validate Cedar content (`CreatePolicy`, `UpdatePolicy`, `CreatePolicyTemplate`, `UpdatePolicyTemplate`). The four copies each re-read `store.validation_mode == "STRICT"`, each re-match `store.schema.as_ref()`, and each spell their own near-identical reason string, so the copies can drift (one already differs from the next only in the words "a policy" vs "a policy template"). One helper on `crate::state::PolicyStore` returning the schema text to validate against — or the `InvalidInput` naming `/policyStoreId` — would leave every response byte-identical, since the only varying part is the noun in the reason sentence.

## Evidence

Copy 1 — src/ops/create_policy.rs:327-343:
```
        let schema_json = match store.validation_mode.as_str() {
            "STRICT" => match store.schema.as_ref() {
                None => {
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::POLICY_STORE_ID,
                        "its validation mode is STRICT and it contains no schema, so there \
                         is nothing to validate a policy against",
                    )));
```
Copy 2 — src/ops/update_policy.rs:299-307:
```
                if store.validation_mode == "STRICT" {
                    match store.schema.as_ref() {
                        None => {
                            return Err(invalid(crate::ops::InvalidInput::new(
                                crate::ops::paths::POLICY_STORE_ID,
                                "its validation mode is STRICT and it contains no schema, so \
                                 there is nothing to validate a policy against",
```
Copy 3 — src/ops/create_policy_template.rs:269-279:
```
        if store.validation_mode == "STRICT" {
            match store.schema.as_ref() {
                None => {
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::POLICY_STORE_ID,
                        "its validation mode is STRICT and it contains no schema, so there \
                         is nothing to validate a policy template against",
```
Copy 4 — src/ops/update_policy_template.rs:262-269:
```
        if store.validation_mode == "STRICT" {
            match store.schema.as_ref() {
                None => {
                    return Err(invalid(crate::ops::InvalidInput::new(
                        crate::ops::paths::POLICY_STORE_ID,
                        "its validation mode is STRICT and it contains no schema, so there \
                         is nothing to validate a policy template against",
```
Existing tests that pin all four responses (they must stay unchanged): tests/all/policy_validation.rs:157 ("a STRICT store with no schema to validate against"), tests/all/policy_update.rs:737-769 ("a STRICT store with no schema"), tests/all/policy_templates.rs:464 `a_strict_store_refuses_a_policy_template_it_cannot_validate`, tests/all/schema.rs:513-537 `a_strict_store_validates_static_policies_against_its_schema`.

## Steps to Reproduce

1. Open src/ops/create_policy.rs:327, src/ops/update_policy.rs:299, src/ops/create_policy_template.rs:269 and src/ops/update_policy_template.rs:262.
2. Observe the same STRICT/no-schema branch and the same `/policyStoreId` ValidationException reason spelled out four times.
3. Change the rule in one place (e.g. the reason wording) and note the other three copies keep the old behaviour.
