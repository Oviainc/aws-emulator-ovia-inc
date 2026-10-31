Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# An account can be linked into two invoice units at once: the modelled mutual-exclusion rule (and its own mutualExclusionError reason) is never applied

_Filed: 2026-10-28T21:57:26Z_
_Operation: CreateInvoiceUnit | Class: validation_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInvoiceUnit` stores `Rule` verbatim and tests nothing about the accounts it names, so this emulator happily holds state the model says cannot exist: the SAME account id linked in two live invoice units at the same time, and the same account id repeated twice inside one rule's `LinkedAccounts` list.

The model describes the resource as "a set of MUTUALLY EXCLUSIVE accounts that correspond to your business entity" and declares `mutualExclusionError` as one of the values of its `ValidationExceptionReason` enum — the refusal reason for exactly this class of request. The handler constructs the two SIBLING reasons from that same enum (`maxInvoiceUnitsExceeded` and `duplicateInvoiceUnit`), each with a comment stating the methodology — "the model's `ValidationExceptionReason` declares X, so the real service refuses Y" — but never applies that reasoning to `mutualExclusionError`: `grep -n mutualExclusion src/` matches only the value list itself, so nothing in the crate can ever answer it.

The consequence is observable through this service's own reads: after two creates naming one account, `ListInvoiceUnits`/`GetInvoiceUnit` report that account inside two different invoice units' rules, and `BatchGetInvoiceProfile`'s membership derivation (`state::linked_accounts`) counts it from both — i.e. one account's costs belong to two business entities at once, which is the opposite of what the shape documents.

The same gap sits on the second write path: `UpdateInvoiceUnit` replaces a unit's rule with the request's `Rule` and applies no overlap test either, so a unit can be moved onto an account another unit already holds. The crate's own pattern for a rule every write of one resource must apply whole is a single function beside the registry (see `state::resolve_marketplace_punch_out_enabled`, whose doc says it lives there so "neither path can drift from the other"), so a fix should cover both writes from one place rather than per handler. I am not proposing any new constant or scope — only that the rule the model states and names an error reason for is applied where the crate already applies its two siblings.

## Evidence

Smithy — `smithy_query(query="shape", shape="InvoiceUnit")`:
  "An invoice unit is a set of mutually exclusive accounts that correspond to your business entity. Invoice units allow you separate Amazon Web Services account costs and configures your invoice for each business entity going forward."

The same sentence restated INSIDE the crate — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_units.rs:70-71:
```
//! * `InvoiceUnits` (InvoiceUnits) — An invoice unit is a set of mutually exclusive accounts
//!   that correspond to your business entity.
```

The modelled refusal reason for this class, present but never constructed — src/ops/types/shared_types.rs:795-796:
```
/// The values `ValidationExceptionReason` accepts on the wire.
pub const VALIDATION_EXCEPTION_REASON_VALUES: &[&str] = &["accountMembershipError", "cannotParse", "duplicateInvoiceUnit", "expiredNextToken", "fieldValidationFailed", "invalidInput", "invalidNextToken", "maxAccountsExceeded", "maxInvoiceUnitsExceeded", "mutualExclusionError", "nonMemberPresent", "other", "taxSettingsError", "unknownOperation"];
```
`grep -n 'mutualExclusion|mutually exclusive' services/service-com-amazonaws-invoicing-invoicing/src` matches only shared_types.rs:796 and list_invoice_units.rs:70 — no handler, helper, comment or test applies or excuses the rule.

The handler's OWN methodology, applied to the two sibling reasons of the same enum — src/ops/create_invoice_unit.rs:179-184:
```
        // The model's `ValidationExceptionReason` declares `maxInvoiceUnitsExceeded`, so
        // the real service refuses a create past its maximum; this emulator's maximum is
        // `MAX_INVOICE_UNITS`. Only the units that EXIST count: a deleted one is history
        // kept for point-in-time reads, not an invoice unit this account holds.
        if store.live_invoice_unit_count() >= crate::state::MAX_INVOICE_UNITS {
```
and create_invoice_unit.rs:211-221:
```
        // `Name` is documented as "a unique name that is distinctive within your Amazon
        // Web Services [account]", and the model's `ValidationExceptionReason` enum
        // carries `duplicateInvoiceUnit` for the collision. ...
        if store.live_invoice_units().any(|unit| unit.name == input.name) {
```

What the same handler does with the accounts — create_invoice_unit.rs:236-250 (no test of `input.rule` against the stored units):
```
        let unit = crate::state::StoredInvoiceUnit {
            arn: arn.clone(),
            name: name.clone(),
            invoice_receiver: input.invoice_receiver,
            created_at,
            current: crate::state::InvoiceUnitVersion {
                description: input.description,
                tax_inheritance_disabled: input.tax_inheritance_disabled,
                rule: input.rule,
                last_modified: created_at,
            },
```

The second write path, equally unchecked — src/ops/update_invoice_unit.rs:196-206:
```
            let next = crate::state::InvoiceUnitVersion {
                description: input
                    .description
                    .or_else(|| unit.current.description.clone()),
                tax_inheritance_disabled: input
                    .tax_inheritance_disabled
                    .unwrap_or(unit.current.tax_inheritance_disabled),
                rule: input.rule.unwrap_or_else(|| unit.current.rule.clone()),
                last_modified: crate::state::now(),
            };
```

Why the boundary cannot be the one refusing it: `smithy_query(query="shape", shape="RuleAccountIdList")` → `{"type": "list", "length": {"min": 0, "max": 1000}, "member": {"target": "AccountIdString", "pattern": "^\\d{12}$"}}` — no `@uniqueItems`, and an overlap with ALREADY-STORED units is a combination of the request and state that no modelled trait can express (the same reason this crate refuses the Coupa punch-out combination and the disabled-plane status in handler code: state.rs:743-756, state.rs:696-701).

The reader that then reports the shared account — src/state.rs:525-536 (`linked_accounts`) walks every live unit and every retained version, so one account id is counted from both units.

Test coverage: nothing pins account exclusivity (`grep -rn 'mutually exclusive' tests/` matches nothing). One fixture currently depends on the gap and would have to vary its account — tests/all/invoice_unit_lifecycle.rs:964-987 creates `MAX_INVOICE_UNITS` LIVE units, every one of them carrying the same linked account, asserting HTTP 200 for each:
```
async fn creating_past_the_maximum_is_refused() {
    ...
    for index in 0..maximum {
        let created = ok(
            &router,
            "CreateInvoiceUnit",
            &json!({
                "Name": format!("unit-{index}"),
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
```
its stated reason (invoice_unit_lifecycle.rs:961-963) is the `maxInvoiceUnitsExceeded` rule, not account sharing, so the sharing is incidental to the fixture rather than pinned behaviour.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateInvoiceUnit` and body `{"Name":"alpha","InvoiceReceiver":"111111111111","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["222222222222"]}}` — note the returned `InvoiceUnitArn`.
3. POST / with `X-Amz-Target: Invoicing.CreateInvoiceUnit` and body `{"Name":"beta","InvoiceReceiver":"333333333333","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["222222222222"]}}`.
4. Observe HTTP 200: account 222222222222 is now linked in two live invoice units at once, although the modelled shape documents an invoice unit as "a set of mutually exclusive accounts" and the model declares `mutualExclusionError` as a `ValidationExceptionReason` value for this class of refusal (nothing in the crate ever constructs it).
5. POST / with `X-Amz-Target: Invoicing.ListInvoiceUnits` and body `{}`: both units are reported, each with 222222222222 in its `Rule.LinkedAccounts`.
6. Second face of the same gap: POST / `Invoicing.CreateInvoiceUnit` with `{"Name":"gamma","InvoiceReceiver":"444444444444","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["555555555555","555555555555"]}}` — accepted, and `GetInvoiceUnit` reports the repeated account verbatim.
7. Third face, on the update path: POST / `Invoicing.UpdateInvoiceUnit` with `{"InvoiceUnitArn":"<alpha arn>","Rule":{"LinkedAccounts":["555555555555"]}}` — accepted, moving alpha onto an account gamma already holds.
