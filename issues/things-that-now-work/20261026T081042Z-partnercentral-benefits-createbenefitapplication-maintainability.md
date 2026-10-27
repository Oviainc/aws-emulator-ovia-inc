Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 23e63f00b11bb74d67049d5cb37769202febdd46

# The catalog-not-found refusal is hand-rolled in three handlers although state::catalog_refusal exists and documents itself as the one helper for every operation that binds Catalog

_Filed: 2026-10-26T08:10:42Z_
_Operation: CreateBenefitApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `23e63f00b11bb74d67049d5cb37769202febdd46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::catalog_refusal` is the single construction of the 404 a request naming an unknown `Catalog` is answered with, and its doc says so explicitly ("One helper for every operation that binds the member"). Eleven handlers call it. Three — `CreateBenefitApplication`, `ListBenefits` and `ListBenefitApplications` — instead build the same `ResourceNotFoundException { message: not_found_message(CATALOG_KIND, &input.catalog) }` inline, because they want the `&Catalog` reference in the same expression. `ListBenefitAllocations` shows the available pattern for exactly that case: it calls `catalog_refusal` first and then re-fetches the catalog (src/ops/list_benefit_allocations.rs:204-244).

The three copies can drift from the helper (which `CATALOG_KIND` is named, or which message builder is used) while each operation's own test keeps passing, and the helper's doc claim would then be false. Routing the three through `catalog_refusal` leaves every response byte-identical — the helper builds the message with the same `not_found_message(CATALOG_KIND, catalog)` call — and leaves every existing test unchanged.

## Evidence

The helper and the policy it states — src/state.rs:1405-1423:
```
    /// The refusal a request naming a catalog this store does not hold is answered with,
    /// or `None` when the catalog exists.
    ///
    /// Every operation binds a required `Catalog` and every resource this service holds
    /// lives inside one, so a request whose catalog does not exist has NO resource to look
    /// up: the missing resource is the catalog, and that is what the answer names — through
    /// [`not_found_message`], the one construction. One helper for every operation that
    /// binds the member, so no operation can report a resource inside a catalog that is
    /// not there.
    pub fn catalog_refusal(
```

Copy 1 — src/ops/create_benefit_application.rs:263-275:
```
            let Some(catalog) = store.catalogs.get(&input.catalog) else {
                return Err(
                    CreateBenefitApplicationErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: crate::state::not_found_message(
                                crate::state::CATALOG_KIND,
                                &input.catalog,
                            ),
```
Copy 2 — src/ops/list_benefits.rs:194-204:
```
        let Some(catalog) = store.catalogs.get(&input.catalog) else {
            return Err(ListBenefitsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::not_found_message(
                        crate::state::CATALOG_KIND,
                        &input.catalog,
                    ),
```
Copy 3 — src/ops/list_benefit_applications.rs:201-213:
```
        let Some(catalog) = store.catalogs.get(&input.catalog) else {
            return Err(
                ListBenefitApplicationsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::not_found_message(
                            crate::state::CATALOG_KIND,
                            &input.catalog,
                        ),
```
Siblings that go through the helper: src/ops/get_benefit.rs:188, src/ops/get_benefit_application.rs:208, src/ops/submit_benefit_application.rs:173, src/ops/cancel_benefit_application.rs:182, src/ops/recall_benefit_application.rs:182, src/ops/update_benefit_application.rs:241, src/ops/amend_benefit_application.rs:486, src/ops/associate_benefit_application_resource.rs:238, src/ops/disassociate_benefit_application_resource.rs:196, src/ops/list_benefit_allocations.rs:204.

Tests that cover all three copies and must stay green: tests/all/unknown_catalog_lookup.rs:153-167 `every_operation_that_binds_a_catalog_reports_the_catalog_it_could_not_find` (asserted against the 14-operation table at tests/all/unknown_catalog_lookup.rs:34-49, which includes CreateBenefitApplication, ListBenefits and ListBenefitApplications) and tests/all/unknown_catalog_lookup.rs:225-242 `a_catalog_this_service_holds_is_never_reported_missing`.

## Steps to Reproduce

1. Open src/state.rs:1405-1423 and note `catalog_refusal` plus its doc claim that it is the one helper for every operation binding `Catalog`.
2. Open src/ops/create_benefit_application.rs:263-275, src/ops/list_benefits.rs:194-204 and src/ops/list_benefit_applications.rs:201-213: each rebuilds `ResourceNotFoundException { message: not_found_message(CATALOG_KIND, &input.catalog) }` by hand instead of calling the helper.
3. Compare src/ops/list_benefit_allocations.rs:204-244, which needs the catalog reference too and still calls `catalog_refusal` before re-fetching it.
4. Change the message construction in the helper only (e.g. the kind it names): `cargo test -p service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice` now fails only for the eleven helper-using operations while the three inline copies keep the old wording, so the copies can diverge unnoticed.
