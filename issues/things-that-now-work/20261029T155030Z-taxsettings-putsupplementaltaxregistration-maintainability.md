Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: e4cbe047d5984815ac113e95db79725169e307b1

# The supplemental-TRN write and delete mutate the store from their handlers, re-implementing the resident-bound/add-vs-replace branch that state.rs already owns twice

_Filed: 2026-10-29T15:50:30Z_
_Operation: PutSupplementalTaxRegistration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `e4cbe047d5984815ac113e95db79725169e307b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other resource in this crate is written and removed through ONE state method whose doc declares itself the single place that happens: `store_registration` ("The one place a `TaxRegistrationEntry` becomes a stored record"), `delete_registration` ("The one place a stored record STOPS being one") and `store_exemptions` ("The one place an exemption becomes a stored record."). Each of those methods also owns the same rule beside its insert: a write that would ADD a record is refused once the store is at its resident bound, while rewriting a record the store already holds adds nothing.

The supplemental tax registration is the exception. `PutSupplementalTaxRegistration`'s handler does the find-or-mint, re-implements that resident-bound branch for a THIRD time, and performs the `BTreeMap::insert` itself; `DeleteSupplementalTaxRegistration`'s handler performs the `BTreeMap::remove` and decides the refusal itself. So the one rule that decides "is this a new record or a replacement, and is there room for a new one?" exists in three copies (state.rs twice, the handler once) and can drift: a change to how the bound is judged — or a second writer of a supplemental TRN, exactly the situation that produced the already-fixed batch/single divergences for registrations — has to be made in a place the other two copies do not reach.

This crate has already settled that principle twice by issue: the page-build rule and the single-account delete decision were both moved out of handlers into one state method. The supplemental lifecycle is what is left. Moving the find-or-mint + bound + insert into a `store_supplemental_registration` state method (and the remove + refusal into a `delete_supplemental_registration` one), beside the three methods that already do this, leaves every response byte-identical and every existing test unchanged.

## Evidence

The rule, as state.rs owns it for the two resources it owns:

services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:701 — "The one place a `TaxRegistrationEntry` becomes a stored record, so `PutTaxRegistration` and the per-account loop of `BatchPutTaxRegistration` cannot write one entry two different ways"
services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:745-747 —
```rust
        if previous_revision.is_none() && self.registrations.len() >= MAX_STORED_REGISTRATIONS {
            return Err(registration_store_full());
        }
```
services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:990 — "The one place an exemption becomes a stored record."
services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1045-1047 —
```rust
        if self.stored_exemption_count() + additional > MAX_STORED_EXEMPTIONS {
            return Err(exemption_store_full());
        }
```
services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:765-766 — "The one place a stored record STOPS being one, so `DeleteTaxRegistration` and the per-account loop of `BatchDeleteTaxRegistration` cannot decide one record's deletion two different ways"

The third copy, in the handler:

services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_supplemental_tax_registration.rs:201-225 —
```rust
        let existing = store
            .find_supplemental_authority(&entry.registration_type, &entry.registration_id);
        let authority_id = match existing {
            Some(authority_id) => authority_id,
            None => {
                if store.supplemental_registrations.len()
                    >= crate::state::MAX_STORED_SUPPLEMENTAL_REGISTRATIONS
                {
                    return Err(PutSupplementalTaxRegistrationErrorModeled::ConflictException(
                        crate::state::supplemental_store_full(),
                    )
                    .into());
                }
                store.mint_supplemental_authority_id()
            }
        };
        ...
        store
            .supplemental_registrations
            .insert(authority_id.clone(), stored);
```

And the delete side, likewise in the handler rather than in state.rs:

services/service-com-amazonaws-taxsettings-taxsettings/src/ops/delete_supplemental_tax_registration.rs:157-168 —
```rust
        if store
            .supplemental_registrations
            .remove(&input.authority_id)
            .is_none()
        {
            return Err(
                DeleteSupplementalTaxRegistrationErrorModeled::ValidationException(
                    crate::state::invalid_authority_id(),
                )
                .into(),
            );
        }
```

These two handlers are the only ones in `src/ops` that touch a store collection directly; every other write goes through a state method (`store.store_registration(...)`, `store.delete_registration(...)`, `store.store_exemptions(...)`).

Test coverage that pins the behaviour a refactor must preserve: `tests/all/supplemental_tax_registrations.rs` (store, replace under the first `authorityId`, delete, and the refusal an unknown `authorityId` gets), `tests/all/shared_trn_settle.rs` (the supplemental record's clock-owed settle), `tests/all/listing_page_bounds.rs` and `tests/all/shared_page_builder.rs` (the listing over `supplemental_registrations`), `tests/all/declared_member_defaults.rs` (the address defaults the stored record carries).

## Steps to Reproduce

1. Read `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:698-760` and `:987-1068`: both writes decide the resident-bound/add-vs-replace question in the state method that performs the insert, and each method's doc declares itself the one place that decision is made.
2. Read `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_supplemental_tax_registration.rs:195-231`: the same decision is re-stated in the handler, which then performs `store.supplemental_registrations.insert(...)` itself.
3. Read `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/delete_supplemental_tax_registration.rs:156-170`: the delete decision and `BTreeMap::remove` are likewise in the handler, unlike `DeleteTaxRegistration`, which calls `store.delete_registration(...)`.
4. grep `src/ops` for `store\.` and confirm these are the only two handlers that mutate a store collection directly instead of calling a state method.
