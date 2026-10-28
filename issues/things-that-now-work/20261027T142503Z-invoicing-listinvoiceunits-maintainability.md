Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# The page-size clamp and the resume-token page walk are written out once per listing handler, five times over

_Filed: 2026-10-27T14:25:03Z_
_Operation: ListInvoiceUnits | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All five paginated listings of this crate implement the same two pieces of logic inline instead of once:

1. Resolve the page size: convert the modelled `MaxResults` to `usize`, fall back to the walk's maximum on a failed conversion, then `clamp(1, MAX_PAGE_SIZE)`.
2. Walk a `BTreeMap` from the decoded token position, cut the page when it holds `page_size` entries, and mint the resume token from the first entry that did not fit.

The copies differ only in the map they walk, the projection they push and the `MAX_PAGE_SIZE` constant, so the RULE (what a page holds, where it cuts, which position the token names) is stated five times and the copies can drift independently — the same drift risk the crate already removed once for the Marketplace-PunchOut precondition by hoisting it into `crate::state` (`resolve_marketplace_punch_out_enabled`, state.rs:669, reached by both writes that apply it).

A shared helper in `crate::state` (beside `encode_page_token`/`decode_page_token`, which the five handlers already share) that takes the requested `MaxResults`, the walk's maximum, the resume position and the map range would leave every response byte-identical and every existing pagination test passing.

## Evidence

Copy 1 — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_units.rs:281-283 and :298-319:
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
...
        let page = match resume_at.as_deref() {
            Some(from) => store.invoice_units.range(from.to_string()..),
            None => store.invoice_units.range::<String, _>(..),
        };
        for unit in page.map(|(_, unit)| unit) {
...
            if invoice_units.len() == page_size {
                next_token = Some(encode_next_token(&unit.arn));
                break;
            }
            invoice_units.push(unit.to_invoice_unit(version));
        }
```

Copy 2 — src/ops/list_procurement_portal_preferences.rs:186-188 and :202-215:
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
...
        let page = match resume_at.as_deref() {
            Some(from) => store.procurement_portal_preferences.range(from.to_string()..),
            None => store.procurement_portal_preferences.range::<String, _>(..),
        };
        for preference in page.map(|(_, preference)| preference) {
            if procurement_portal_preferences.len() == page_size {
                next_token = Some(crate::state::encode_page_token(
                    NEXT_TOKEN_PREFIX,
                    &preference.preference.procurement_portal_preference_arn,
                ));
                break;
            }
            procurement_portal_preferences.push(preference.to_summary());
        }
```

Copies 3-5, same two pieces verbatim: src/ops/list_procurement_portals.rs:188-190 and :198-211; src/ops/list_procurement_portal_suppliers.rs:207-209 and :230-240; src/ops/list_invoice_summaries.rs:362-366 and :407-420 (the clamp variant for a member with no `@default`: `.and_then(|requested| usize::try_from(requested).ok()).unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)`).

The precedent for hoisting a rule the handlers share — src/state.rs:662-668:
```
/// Both the DEFAULT and the combination that is refused live here, beside the preference
/// registry, rather than in one operation's module, for the reason [`PORTAL_COUPA`]
/// states: they are one rule about the RESOURCE, which every write that sets a
/// preference's Marketplace PunchOut configuration ... applies
/// whole, so neither path can drift from the other.
```

Existing tests that pin the behaviour and must stay unchanged: tests/all/invoice_unit_lifecycle.rs:460-512 (every page size, pages concatenate in order, expected page count); tests/all/invoice_summaries.rs:436-482 and :487-510; tests/all/procurement_portals.rs:290-345; tests/all/procurement_portal_suppliers.rs:431-459 and :524-541; tests/all/procurement_portal_preferences.rs:468-534.

## Steps to Reproduce

1. Open src/ops/list_invoice_units.rs at lines 281-283 and src/ops/list_procurement_portal_preferences.rs at lines 186-188: the same `usize::try_from(input.max_results).unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)` computation, character for character.
2. Compare the walk at list_invoice_units.rs:298-319 with list_procurement_portal_preferences.rs:202-215, list_procurement_portals.rs:198-211 and list_procurement_portal_suppliers.rs:230-240: the same `range(from.to_string()..)` / `range::<String, _>(..)` branch, the same `if out.len() == page_size { next_token = Some(encode(..)); break; }` cut, five times.
3. Note that src/state.rs already owns the shared half of this rule (`encode_page_token`, `decode_page_token`, state.rs:1196-1215), so the remaining five copies have no reason to live in the handlers.
4. Run the pagination tests named in the evidence before and after any hoist: every page, every token and every response body must be identical.
