Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# The Coupa Marketplace-PunchOut precondition is branched twice, in the create and in the put handler

_Filed: 2026-10-27T08:21:59Z_
_Operation: CreateProcurementPortalPreference | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that gates `MarketplacePunchOutEnabled` for Coupa is implemented twice: the three-term predicate (`enabled` && the portal is `COUPA` && no `MarketplacePunchOutPreference`) is spelled out in `CreateProcurementPortalPreference::handle` and again in `PutProcurementPortalPreference::handle`. Only the MESSAGE half of the rule was hoisted into `crate::state` (`marketplace_punch_out_preference_required`), so the two branches can drift (e.g. a second portal joining the rule, or the `unwrap_or(false)` default changing) while both keep calling the same shared formatter and so keep answering identically — a drift no test would catch until the two paths disagree.

`crate::state` already states that this rule belongs to the resource rather than to one operation's module, which is the same argument for the predicate as for the message. Hoisting the predicate beside the message leaves every response byte-identical and both existing tests passing.

## Evidence

Copy 1 — services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs:208-219:
```
        let marketplace_punch_out_enabled = input.marketplace_punch_out_enabled.unwrap_or(false);
        if marketplace_punch_out_enabled
            && input.procurement_portal_name == crate::state::PORTAL_COUPA
            && input.marketplace_punch_out_preference.is_none()
        {
            return Err(CreateProcurementPortalPreferenceErrorModeled::ValidationException(
                crate::state::marketplace_punch_out_preference_required(
```
Copy 2 — services/service-com-amazonaws-invoicing-invoicing/src/ops/put_procurement_portal_preference.rs:231-246:
```
            let marketplace_punch_out_enabled = input.marketplace_punch_out_enabled.unwrap_or(false);
            // The same Marketplace PunchOut rule the create applies, …
            if marketplace_punch_out_enabled
                && stored.preference.procurement_portal_name == crate::state::PORTAL_COUPA
                && input.marketplace_punch_out_preference.is_none()
            {
                return Err(PutProcurementPortalPreferenceErrorModeled::ValidationException(
                    crate::state::marketplace_punch_out_preference_required(
```
The crate's own argument for one home — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:640-643:
```
/// It lives beside the preference registry rather than inside one operation's module
/// because the combination it gates is a rule about the RESOURCE: every write that sets
/// a preference's Marketplace PunchOut configuration — the create and the update —
/// refuses the same combination.
```
(only `marketplace_punch_out_preference_required`, state.rs:658-672, was moved there).

Tests that pin both copies and must stay unchanged: tests/all/procurement_portal_preferences.rs:750-795 `coupa_punch_out_requires_its_preference` (the create path, including the "Coupa, disabled", "Coupa, omitted" and other-portal rows) and tests/all/refused_procurement_portal_writes.rs:235-239 `update_missing_required_punch_out_config` (the put path).

## Steps to Reproduce

1. Read services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs:208-219 and services/service-com-amazonaws-invoicing-invoicing/src/ops/put_procurement_portal_preference.rs:231-246: the same three-term predicate appears in both.
2. Read services/service-com-amazonaws-invoicing-invoicing/src/state.rs:640-672: the state module hosts the message for this rule and states that the rule belongs to the resource, not to one operation.
3. Change one copy (e.g. add a portal to the condition) and observe that no existing test fails for the other copy, because each operation is covered by its own test.
