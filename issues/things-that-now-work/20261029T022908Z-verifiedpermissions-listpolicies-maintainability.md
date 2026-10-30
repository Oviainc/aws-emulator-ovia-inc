Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 77670ec594ee56220d3fb04f56c8d32fdb5a5e9f

# The pagination rule (cursor decode, page-size default/clamp, page loop, cursor mint) is implemented four times, once per list handler

_Filed: 2026-10-29T02:29:08Z_
_Operation: ListPolicies | Class: maintainability_

> _Note: this issue was identified against the code base at commit `77670ec594ee56220d3fb04f56c8d32fdb5a5e9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has four `@paginated` list handlers, and each one carries its own private copy of the SAME pagination logic — not a repeated literal, but the rule itself:

1. decode `nextToken` through `crate::state::decode_page_token`, refusing an undecodable cursor with a `ValidationException` naming `crate::ops::paths::NEXT_TOKEN` with the message "it is not a cursor this service issued";
2. resume the `BTreeMap` walk at the first key `>=` the decoded cursor (`resume_from.as_ref().is_none_or(|from| key.as_str() >= from.as_str())`);
3. resolve the page size: the operation's default when `maxResults` is absent, otherwise `usize::try_from(n).unwrap_or(<default>).clamp(1, crate::state::MAX_PAGE_SIZE)`;
4. fill the page, and on overflow mint the cursor from the key the page stopped at (`encode_page_token(key)`) and break.

The four copies live in `list_policies.rs`, `list_identity_sources.rs`, `list_policy_templates.rs` and `list_policy_store_aliases.rs`, and differ only in the item type, the `BTreeMap` being walked, the per-operation default constant (`DEFAULT_PAGE_SIZE` vs `ALIAS_DEFAULT_PAGE_SIZE`) and the operation's own error enum. Because the rule is copied rather than shared, the copies can drift: a change to how a cursor is refused, where the page boundary is taken, or how `maxResults` is clamped has to be made four times, and the still-stubbed `ListPolicyStores` (which is `@paginated` over the same `policy_stores` `BTreeMap`) will need a fifth. The crate already centralises the two *pieces* this logic is built from (`encode_page_token`/`decode_page_token`, `DEFAULT_PAGE_SIZE`/`ALIAS_DEFAULT_PAGE_SIZE`/`MAX_PAGE_SIZE` all live in `state.rs` with the stated reason "ONE constant for every list operation whose model states that figure, so they cannot page differently") — it is the LOGIC that reads them that is duplicated.

This is a structural finding only: every response byte and every existing test must stay exactly as they are. A generic helper over `(&BTreeMap<String, T>, Option<&str> /*cursor*/, Option<i32> /*maxResults*/, usize /*default*/, filter)` returning `(Vec<Item>, Option<String>)` plus one shared cursor-decode that hands back an `InvalidInput` for each handler to lift into its own error enum reproduces all four call sites verbatim.

## Evidence

Copy 1 — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/list_policies.rs:149-161 and 195-212:
```
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListPoliciesErrorModeled::ValidationException(crate::ops::validation_exception([
                    crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    ),
                ]))
            })?),
        };
...
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_PAGE_SIZE, |n| {
                usize::try_from(n)
                    .unwrap_or(crate::state::DEFAULT_PAGE_SIZE)
                    .clamp(1, crate::state::MAX_PAGE_SIZE)
            });
        let mut policies: Vec<PolicyItem> = Vec::new();
        let mut next_token = None;
        for (policy_id, policy) in matching {
            if policies.len() == page_size {
                next_token = Some(crate::state::encode_page_token(policy_id));
                break;
            }
            policies.push(policy_item(policy));
        }
```

Copy 2 — src/ops/list_identity_sources.rs:154-164 and 198-215 (same rule, same words, different enum/item):
```
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListIdentitySourcesErrorModeled::ValidationException(
                    crate::ops::validation_exception([crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    )]),
                )
            })?),
        };
...
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_PAGE_SIZE, |n| {
                usize::try_from(n)
                    .unwrap_or(crate::state::DEFAULT_PAGE_SIZE)
                    .clamp(1, crate::state::MAX_PAGE_SIZE)
            });
        let mut identity_sources: Vec<IdentitySourceItem> = Vec::new();
        let mut next_token = None;
        for (source_id, source) in matching {
            if identity_sources.len() == page_size {
                next_token = Some(crate::state::encode_page_token(source_id));
                break;
            }
            identity_sources.push(identity_source_item(source));
        }
```

Copy 3 — src/ops/list_policy_templates.rs:147-159 and 189-206 (identical, with `ListPolicyTemplatesErrorModeled` / `PolicyTemplateItem`).

Copy 4 — src/ops/list_policy_store_aliases.rs:159-169 and 181-215 (identical cursor-decode block; the page size differs only in the default constant):
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(ALIAS_DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
```

The pieces the copies read are already single-sourced, which is what makes the surrounding logic the odd one out — src/state.rs:255-265: "ONE constant for every list operation whose model states that figure, so they cannot page differently", and src/state.rs:1274-1283 (`encode_page_token` / `decode_page_token`).

A fifth copy is pending: `ListPolicyStores` is `@paginated(inputToken=nextToken, outputToken=nextToken, items=policyStores)` (OPERATIONS.md:76) and is still the scaffold.

Test coverage that must stay green and unchanged (each copy has its own suite, so a shared helper is verified four times over):
- tests/all/policy_store_authorization.rs:1015-1136 (ListPolicies paging, forged `nextToken` -> 400) and 1838-1906 (`no maxResults` -> DEFAULT_PAGE_SIZE, "maxResults above the cap" -> MAX_PAGE_SIZE);
- tests/all/list_identity_sources.rs:67-91, 245-299, 303-383, 492-563;
- tests/all/list_policy_templates.rs:64-88, 216-271, 275-340, 427-498;
- tests/all/list_policy_store_aliases.rs:77-114, 135-251, 251-292, 362;
- tests/all/policy_validation.rs:188-189 (`"nextToken": "notacursor"` -> `/nextToken`).

## Steps to Reproduce

1. Open src/ops/list_policies.rs:149-161 and src/ops/list_identity_sources.rs:154-164 side by side: the cursor-decode block, including the refusal message "it is not a cursor this service issued" and the `paths::NEXT_TOKEN` pointer, is the same rule written twice.
2. Compare src/ops/list_policies.rs:195-212, src/ops/list_identity_sources.rs:198-215 and src/ops/list_policy_templates.rs:189-206: the page-size resolution (`map_or(DEFAULT_PAGE_SIZE, ...).clamp(1, MAX_PAGE_SIZE)`), the page loop and the `encode_page_token(key)` cursor mint are character-for-character the same apart from the item type.
3. Compare src/ops/list_policy_store_aliases.rs:159-169 and 181-215: the fourth copy, differing only in the default constant (`ALIAS_DEFAULT_PAGE_SIZE`).
4. Note OPERATIONS.md:76 — the still-stubbed `ListPolicyStores` is `@paginated` over the same `policy_stores` BTreeMap, so implementing it adds a fifth copy of the same rule.
5. Change the rule in one place only (for example the cursor-refusal message or the page boundary) and observe that the other three handlers keep the old behaviour — the drift this duplication permits.
