Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6c0e2feb75d7e0fede15568cd8941f97f9b3d17f

# The single-account delete decision is hand-copied in the batch delete path instead of being one shared state method like the write path

_Filed: 2026-10-29T09:37:13Z_
_Operation: DeleteTaxRegistration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6c0e2feb75d7e0fede15568cd8941f97f9b3d17f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Deleting ONE account's tax registration is a three-step decision: a 404 when the account holds no record, a 400 when the record's jurisdiction is carved out of the API delete, otherwise remove the key. That decision is implemented twice in this crate's own code — once in `DeleteTaxRegistration`'s handler and once, re-derived, in `BatchDeleteTaxRegistration`'s per-account loop — even though only its middle step (`crate::state::check_registration_deletable`) was ever folded into a shared helper.

The batch copy's own comment asserts the two paths cannot disagree ("Exactly what this account's own single-account delete would have answered, so the two delete paths cannot disagree about one record"), and the test suite states the same property ("the per-account fan-out of the single-account delete"), but nothing in the code enforces it: the not-found branch and the `registrations.remove(...)` call are duplicated source, so a later change to what a delete of one account does (another jurisdiction carve-out that must also leave something behind, a release of the account's document tokens, a different refusal for a record in some state) has two places to touch and only one of them is named by the single-account handler.

This is the consolidation the crate already made on the WRITE side: `state::store_registration` is documented as "the one place a `TaxRegistrationEntry` becomes a stored record, so `PutTaxRegistration` and the per-account loop of `BatchPutTaxRegistration` cannot write one entry two different ways", and `BatchPutTaxRegistration` calls it rather than re-deriving the write. The delete side has no equivalent. Folding the whole decision into one state method that answers which refusal applies (each handler rendering it on its own channel — a modelled error, or a per-account `errors` row) leaves every response byte-identical and every existing assertion unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/delete_tax_registration.rs:158-172`:
```
        // An account holding nothing has nothing to delete, whatever jurisdiction
        // the carve-out is about: that is this operation's 404.
        let Some(stored) = store.registrations.get(&account_id) else {
            return Err(DeleteTaxRegistrationErrorModeled::ResourceNotFoundException(
                crate::state::no_tax_registration(),
            )
            .into());
        };

        // A record this API cannot remove is left exactly as it was, so every
        // later read still reports it.
        crate::state::check_registration_deletable(stored)
            .map_err(DeleteTaxRegistrationErrorModeled::ValidationException)?;

        store.registrations.remove(&account_id);
```

Copy 2 — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/batch_delete_tax_registration.rs:171-193`, the same three steps re-derived, with the claim that it is the same decision:
```
            // Exactly what this account's own single-account delete would have
            // answered, so the two delete paths cannot disagree about one
            // record: the code is the modelled error that delete raises and the
            // record is left in place whenever one is raised.
            let refused: Option<(&str, String)> = match store.registrations.get(account_id) {
                None => Some((
                    "ResourceNotFoundException",
                    crate::state::no_tax_registration().message,
                )),
                Some(stored) => crate::state::check_registration_deletable(stored)
                    .err()
                    .map(|refusal| ("ValidationException", refusal.message)),
            };
            match refused {
                Some((code, message)) => errors.push(BatchDeleteTaxRegistrationError2 { ... }),
                None => {
                    store.registrations.remove(account_id);
                }
            }
```

Only the middle step is shared — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:3184-3187`:
```
/// The one place both delete paths — `DeleteTaxRegistration` and the per-account
/// loop of `BatchDeleteTaxRegistration` — decide whether a record is removable
/// through the API, so the two operations cannot disagree about one record.
```
while the write side consolidated the WHOLE decision — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:685-689`:
```
    /// The one place a `TaxRegistrationEntry` becomes a stored record, so
    /// `PutTaxRegistration` and the per-account loop of
    /// `BatchPutTaxRegistration` cannot write one entry two different ways: the
    /// write replaces the account's record and bumps its revision, which the
    /// document access tokens carry.
```

Tests a consolidation must leave unchanged (they already treat the two paths as one rule): `tests/all/batch_tax_registration.rs:490-492` — "RULE: a batch delete removes the registration of every account it names, and reports an account that held none against that account rather than failing the call — the per-account fan-out of the single-account delete."; `tests/all/non_api_delete_jurisdictions.rs:13-14` — "both delete paths (`DeleteTaxRegistration` and `BatchDeleteTaxRegistration`) apply it", with its single-account rule at line 204 and its batch rule at line 272; `tests/all/tax_registration_lifecycle.rs:707`; `tests/all/missing_resource_errors.rs:289`; `tests/all/repeated_account_ids.rs:147`.

## Steps to Reproduce

1. Open src/ops/delete_tax_registration.rs:158-172 and src/ops/batch_delete_tax_registration.rs:171-193 side by side: both decide "no record -> not found", "check_registration_deletable -> validation refusal", "otherwise registrations.remove(...)" in their own source.
2. Compare with the write paths: src/ops/batch_put_tax_registration.rs:259 calls the same state::store_registration that src/ops/put_tax_registration.rs:322 calls, so the write decision exists once (state.rs:685-689 states that this is deliberate).
3. Note that only the middle step of the delete decision was shared (state.rs:3184-3187), so the not-found branch and the removal remain two copies that can drift while the batch handler's comment and tests/all/batch_tax_registration.rs:490-492 both assert they are one decision.
4. Confirm nothing a caller sees is at stake: both copies already produce the same message (crate::state::no_tax_registration().message / the same ValidationException message) and the same post-state, so moving the decision into one state method keeps every response and every assertion in the listed tests unchanged.
