Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# Resolve the request's catalog once: four handlers look it up twice and carry a dead InternalServerException branch

_Filed: 2026-10-28T20:30:39Z_
_Operation: ListBenefitApplications | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` owns the catalog resolution: `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::catalog_refusal` performs the `self.catalogs.contains_key(catalog)` lookup and builds the modelled `ResourceNotFoundException`, and its doc states it is "One helper for every operation that binds the member".

Four handlers nevertheless resolve the SAME key a SECOND time immediately after calling that helper — `store.catalogs.get(&input.catalog)` — and each copy carries its own `else` branch answering the modelled `InternalServerException` for a case its own comment declares impossible ("present, since the lookup above found it and nothing since could have removed it (the store is locked for the whole body)"). So the same computation (which catalog this request names, and whether the store holds it) is written twice per handler and the second step is copied four times, each with an unreachable arm that would silently convert a drifted key (a copy fetching a different member) into a 500 instead of a compile error.

The four copies can drift from `catalog_refusal` and from each other: nothing ties the key the refusal checked to the key the re-fetch reads. One accessor that resolves the catalog and hands back either the stored `Catalog` or the refusal `catalog_refusal` already builds would be called once per handler, delete the four dead branches, and leave every response byte and every existing test unchanged (the dead arm is unreachable, so no response path changes).

## Evidence

The helper that owns the lookup — src/state.rs:1534-1549:
    /// Every operation binds a required `Catalog` and every resource this service holds
    /// lives inside one, so a request whose catalog does not exist has NO resource to look
    /// up: the missing resource is the catalog, and that is what the answer names — through
    /// [`not_found_message`], the one construction. One helper for every operation that
    /// binds the member, so no operation can report a resource inside a catalog that is
    /// not there.
    pub fn catalog_refusal(
        &self,
        catalog: &str,
    ) -> Option<crate::ops::types::ResourceNotFoundException> {
        (!self.catalogs.contains_key(catalog)).then(|| {
            crate::ops::types::ResourceNotFoundException {
                message: not_found_message(CATALOG_KIND, catalog),
            }
        })
    }

Copy 1 — src/ops/list_benefit_applications.rs:204-220 (the refusal, then the same key resolved again):
        if let Some(missing) = store.catalog_refusal(&input.catalog) {
            return Err(
                ListBenefitApplicationsErrorModeled::ResourceNotFoundException(missing).into(),
            );
        }
        // The catalog itself: present, since the lookup above found it and nothing since
        // could have removed it (the store is locked for the whole body).
        let Some(catalog) = store.catalogs.get(&input.catalog) else {
            return Err(
                ListBenefitApplicationsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: crate::state::STORE_UNAVAILABLE.to_string(),
                    },
                )
                .into(),
            );
        };

Copy 2 — src/ops/list_benefits.rs:197-209:
        if let Some(missing) = store.catalog_refusal(&input.catalog) {
            return Err(ListBenefitsErrorModeled::ResourceNotFoundException(missing).into());
        }
        // The catalog itself: present, since the lookup above found it and nothing since
        // could have removed it (the store is locked for the whole body).
        let Some(catalog) = store.catalogs.get(&input.catalog) else {
            return Err(ListBenefitsErrorModeled::InternalServerException(
                InternalServerException {
                    message: crate::state::STORE_UNAVAILABLE.to_string(),
                },
            )
            .into());
        };

Copy 3 — src/ops/list_benefit_allocations.rs:226-235:
        // The catalog itself: present, since the lookup above found it and nothing since
        // could have removed it (the store is locked for the whole body).
        let Some(catalog) = store.catalogs.get(&input.catalog) else {
            return Err(ListBenefitAllocationsErrorModeled::InternalServerException(
                InternalServerException {
                    message: crate::state::STORE_UNAVAILABLE.to_string(),
                },
            )
            .into());
        };

Copy 4 — src/ops/create_benefit_application.rs:332-352 (`catalog_refusal` at :332, the re-fetch at :343):
        if let Some(missing) = store.catalog_refusal(&input.catalog) {
...
            let Some(catalog) = store.catalogs.get(&input.catalog) else {
                return Err(
                    CreateBenefitApplicationErrorModeled::InternalServerException(
                        InternalServerException {
                            message: crate::state::STORE_UNAVAILABLE.to_string(),
                        },
                    )
                    .into(),
                );
            };

Test coverage that must stay unchanged: tests/all/unknown_catalog_lookup.rs:3-19 and :34-49 drive "every operation the model binds a `Catalog` member on" — including `ListBenefitApplications`, `ListBenefits`, `ListBenefitAllocations` and `CreateBenefitApplication` — with catalog names the modelled pattern admits, asserting the 404 `ResourceNotFoundException` that names the catalog; tests/all/pagination_cursor_issuance.rs and tests/all/benefit_allocations.rs exercise the success path of the three listings through the same two-step lookup.

## Steps to Reproduce

1. Read src/state.rs:1534-1549 and note `catalog_refusal` already performs `self.catalogs.contains_key(catalog)` and documents itself as the one helper for every operation that binds `Catalog`.
2. Read src/ops/list_benefit_applications.rs:204-220, src/ops/list_benefits.rs:197-209, src/ops/list_benefit_allocations.rs:204-235 and src/ops/create_benefit_application.rs:332-352: each calls `catalog_refusal` and then resolves the same `input.catalog` again with `store.catalogs.get(&input.catalog)`.
3. Observe that every one of the four re-fetches carries an `else` arm answering the modelled `InternalServerException` for a state its own comment calls impossible (the store is locked for the whole handler body), so the branch is dead code repeated four times.
4. Confirm no response depends on the dead arm: run tests/all/unknown_catalog_lookup.rs (404 path) and tests/all/pagination_cursor_issuance.rs (success path) — both only exercise the refusal and the resolved-catalog paths.
