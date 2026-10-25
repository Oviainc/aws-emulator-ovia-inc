Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# A request naming a catalog that does not exist is answered 'BenefitApplication `…` not found' instead of the catalog lookup the sibling handlers report

_Filed: 2026-10-25T20:41:58Z_
_Operation: GetBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this service binds a required `Catalog` member. Six handlers (CreateBenefitApplication, GetBenefit, GetBenefitAllocation, ListBenefits, ListBenefitApplications, ListBenefitAllocations) check that the catalog exists first and answer ResourceNotFoundException naming `CATALOG_KIND` and the catalog the request supplied. GetBenefitApplication skips that check: it goes straight to `store.application(&input.catalog, &input.identifier)`, which returns None for an unknown catalog, so the 404 names the BenefitApplication and quotes the *identifier* even though the resource that was missing is the catalog. The same omission is in the seven application-identified mutators that share the `application_mut` lookup (Update, Amend, Submit, Cancel, Recall, Associate, Disassociate), so a caller that mistypes `Catalog` is told the application does not exist in a catalog that itself does not exist.

This contradicts the one construction the crate centralizes for this answer, whose doc says the message exists so a client 'learns WHICH resource was missing', and the rule the crate's own test states for every lookup this service does.

## Evidence

The handler that skips the check — src/ops/get_benefit_application.rs:204-217:
```
        let store = state.lock();
        let Some(application) = store.application(&input.catalog, &input.identifier) else {
            return Err(
                GetBenefitApplicationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::not_found_message(
                            crate::state::APPLICATION_KIND,
                            &input.identifier,
                        ),
```
(`store.application` is state.rs:1421-1424, which resolves through `self.catalogs.get(catalog)?` and so yields None for an unknown catalog.)

The sibling handlers that do check — src/ops/create_benefit_application.rs:263-271:
```
            let Some(catalog) = store.catalogs.get(&input.catalog) else {
                return Err(
                    CreateBenefitApplicationErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: crate::state::not_found_message(
                                crate::state::CATALOG_KIND,
                                &input.catalog,
```
src/ops/get_benefit.rs:185-195:
```
        if !store.catalogs.contains_key(&input.catalog) {
            return Err(GetBenefitErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::not_found_message(
                        crate::state::CATALOG_KIND,
                        &input.catalog,
```
the same check again at src/ops/get_benefit_allocation.rs:264-274, src/ops/list_benefits.rs:194-204, src/ops/list_benefit_applications.rs:201-213 and src/ops/list_benefit_allocations.rs:201-211.

The rule the crate states for this message — state.rs:2268-2277:
```
/// AWS's "not found" message: the resource that was looked up, then the identifier the
/// request named it with, backtick-quoted — e.g. ``Catalog `Sandbox` not found``.
///
/// One construction for every lookup this service does, so a client reading the message
/// learns WHICH resource was missing from the one place the sentence is written.
```
and the test that states it — tests/all/service_error_rules.rs:976-978:
```
/// Every lookup this service does: the 404 names the resource and the identifier the
/// request named it with, backtick-quoted, whichever spelling the modelled identifier
/// pattern admits.
```
That test only exercises the unknown catalog on create and the two listings (rows at tests/all/service_error_rules.rs:990-1017); every application-identified row (lines 1040-1083) passes the valid `CATALOG`, so the unknown-catalog case on these handlers is unpinned.

## Steps to Reproduce

1. Start the emulator with the partner entitlement granted.
2. Call ListBenefits with {"Catalog": "not-a-real-catalog-34556872"} -> 404 ResourceNotFoundException, message 'Catalog `not-a-real-catalog-34556872` not found'.
3. Call GetBenefitApplication with {"Catalog": "not-a-real-catalog-34556872", "Identifier": "benappl-00000000000001"} -> 404 ResourceNotFoundException, message 'BenefitApplication `benappl-00000000000001` not found': the missing catalog is never named.
4. Repeat with SubmitBenefitApplication / UpdateBenefitApplication / CancelBenefitApplication / RecallBenefitApplication / Associate- and DisassociateBenefitApplicationResource against the same unknown catalog: each reports the application rather than the catalog, while the create and the listings report the catalog.
