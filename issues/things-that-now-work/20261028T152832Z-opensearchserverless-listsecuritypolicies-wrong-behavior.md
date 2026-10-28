Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 48bb9604e330d1bf8edda7143fefbdaecc9daf20

# Read a policy document's Rules/Resource the same way in the listing filter as in the encryption lookup

_Filed: 2026-10-28T15:28:32Z_
_Operation: ListSecurityPolicies | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `48bb9604e330d1bf8edda7143fefbdaecc9daf20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate reads a stored security policy document twice, with two different notions of the same documented shape:

* `collect_policy_resources` (the resources `ListSecurityPolicies`' `resource` filter matches against) requires `Rules` to be a JSON array AND each rule's `Resource` to be a JSON array; anything else contributes no resources at all.
* `statement_covers` / `rule_covers` (the encryption lookup `CreateCollection` performs through `policy_encryption_for`) normalise a non-array `Rules` and a non-array `Resource` into a one-element list before matching.

So for one and the same stored policy whose rule writes `"Resource": "collection/foo"` (or whose `Rules` is a single object rather than a list), the service holds two contradictory views: the policy DOES cover `collection/foo` (CreateCollection accepts the collection on the strength of it and stores the key it assigns), yet `ListSecurityPolicies(type="encryption", resource=["collection/foo"])` reports the policy as applying to nothing and leaves it out of the narrowed listing. A caller that uses the filtered listing to find which policy governs a collection is told no policy does, while the create path has already used that policy.

The filter path is the one that skips the normalisation its sibling already implements in this same module.

## Evidence

The strict reader (the listing's resource filter): "let Some(Document::Array(rules)) = members.get(\"Rules\") else {\n                return;\n            };" — src/state.rs:1242-1244, and "let Some(Document::Array(named)) = rule.get(\"Resource\") else {\n                    continue;\n                };" — src/state.rs:1249-1251. Reached from the handler via `.map(crate::state::SecurityPolicyRecord::listing_entry)` (src/ops/list_security_policies.rs:186) -> `resources: policy_document_resources(&self.policy)` (src/state.rs:1184) -> `crate::state::resource_filtered(candidates, filter)` (src/ops/list_security_policies.rs:194).

The lenient reader, in the same module, for the same documents: "let rules: Vec<&Document> = match members.get(\"Rules\") {\n        Some(Document::Array(items)) => items.iter().collect(),\n        Some(single) => vec![single],\n        None => Vec::new(),\n    };" — src/state.rs:1885-1889, and "let patterns: Vec<&Document> = match members.get(\"Resource\") {\n        Some(Document::Array(items)) => items.iter().collect(),\n        Some(single) => vec![single],\n        None => Vec::new(),\n    };" — src/state.rs:1913-1917. Reached from `CreateCollection` via `let policy_encryption = store.policy_encryption_for(&input.name);` (src/ops/create_collection.rs:261) -> `covering_statement(&record.policy, &resource)` (src/state.rs:2280).

The module asserts the two cannot disagree: "the match itself is [`resource_pattern_matches`], the one `*` walk this module has, so a policy's rule and a listing's filter cannot read the same pattern differently." — src/state.rs:1897-1899. For a scalar `Resource` they never reach that shared walk at all: the filter path drops the rule before matching.

Test coverage: every policy document in the tests writes `Resource` as an array — tests/all/security_policy_listing.rs:120-144, :348-373 and tests/all/encryption_indexes_and_tagging.rs:201, :269 — so no test pins either reading of the scalar form, and nothing justifies the divergence.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="scalar-rule", policy='{"Rules":[{"ResourceType":"collection","Resource":"collection/foo"}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="foo") with NO encryptionConfig -> 200: the create accepts it, so the service treats the policy above as covering `collection/foo` (src/ops/create_collection.rs:261-274).
3. ListSecurityPolicies(type="encryption", resource=["collection/foo"]) -> the `scalar-rule` policy is absent from `securityPolicySummaries`, although the service just used it to encrypt `collection/foo`.
4. ListSecurityPolicies(type="encryption") with no `resource` filter -> the policy IS reported, confirming the omission comes from the filter's stricter reading of the same document.
