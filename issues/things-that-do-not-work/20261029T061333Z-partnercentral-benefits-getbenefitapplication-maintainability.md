Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2a30e9f7a31e09aa2b907716a7c9b6be0706cfea

# The application-not-found refusal is hand-rolled in eight handlers although the sibling catalog lookup is single-construction

_Filed: 2026-10-29T06:13:33Z_
_Operation: GetBenefitApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2a30e9f7a31e09aa2b907716a7c9b6be0706cfea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states an explicit single-construction policy for resource-lookup refusals and implements it for the CATALOG resource: `state::catalog` returns `Result<&Catalog, ResourceNotFoundException>` and `state::catalog_refusal` exposes "[`Self::catalog`]'s refusal alone, for the operations that only need to know whether the catalog is there", so all eight handlers that need it write one line (`store.catalog_refusal(&input.catalog)`).

The BENEFIT APPLICATION lookup has no such counterpart: `state::application` and `state::application_mut` return a bare `Option`, and each of the eight handlers that resolve an application therefore re-implements the refusal itself — picking `state::APPLICATION_KIND`, pairing it with the request's own identifier member, and wrapping the result in `ResourceNotFoundException`. Eight copies of the same rule can drift independently: a copy could pass the wrong kind constant, or the resolved stored id instead of the identifier the request named (the message is required to quote the latter — `service_error_rules.rs:1101-1105` asserts `message.contains(&format!("`{identifier}`"))`), and nothing would make the other seven disagree.

The fix is the one this crate already applied to the catalog path (see the fixed issue "The catalog-not-found refusal is hand-rolled in three handlers although state::catalog_refusal exists"): one `state`-level construction of the application refusal that the eight handlers lift into their own per-operation error enum. Responses are byte-identical — the message is already built by the single `state::not_found_message` — and every existing test keeps passing.

## Evidence

The policy, stated in this crate's own state module:

- `src/state.rs:1598-1606` — "The refusal a request naming a catalog this store does not hold is answered with, or `None` when the catalog exists: [`Self::catalog`]'s refusal alone, for the operations that only need to know whether the catalog is there." / `pub fn catalog_refusal(&self, catalog: &str) -> Option<crate::ops::types::ResourceNotFoundException> { self.catalog(catalog).err() }`
- `src/state.rs:1582-1586` (on `catalog`) — "ONE resolution for every operation that binds the member: the lookup and the refusal it fails with are the same step ... so an operation that goes on to read the catalog reads the record this resolution found rather than resolving the key again"
- `src/state.rs:2825-2826` (on `not_found_message`) — "One construction for every lookup this service does, so a client reading the message learns WHICH resource was missing from the one place the sentence is written."

The application lookup has no refusal counterpart:

- `src/state.rs:1629-1633` — "/// The application `identifier` names, for a reporting operation." / `pub fn application(&self, catalog: &str, identifier: &str) -> Option<&BenefitApplication> { ... }`
- `src/state.rs:1811-1818` — `pub fn application_mut(&mut self, catalog: &str, identifier: &str) -> Option<&mut BenefitApplication> { ... }`

So the refusal is written out eight times. Two of the copies, verbatim:

`src/ops/get_benefit_application.rs:214-226`:
```rust
let Some(application) = store.application(&input.catalog, &input.identifier) else {
    return Err(
        GetBenefitApplicationErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: crate::state::not_found_message(
                    crate::state::APPLICATION_KIND,
                    &input.identifier,
                ),
            },
        )
        .into(),
    );
};
```

`src/ops/associate_benefit_application_resource.rs:242-256`:
```rust
let Some(application) =
    store.application_mut(&input.catalog, &input.benefit_application_identifier)
else {
    return Err(
        AssociateBenefitApplicationResourceErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: crate::state::not_found_message(
                    crate::state::APPLICATION_KIND,
                    &input.benefit_application_identifier,
                ),
            },
        )
        .into(),
    );
};
```

The other six copies sit at `src/ops/amend_benefit_application.rs:536-548`, `src/ops/cancel_benefit_application.rs:231-243`, `src/ops/disassociate_benefit_application_resource.rs:205-219`, `src/ops/recall_benefit_application.rs:280-292`, `src/ops/submit_benefit_application.rs:179-191` and `src/ops/update_benefit_application.rs:343-355` (each naming `crate::state::APPLICATION_KIND`, per `grep APPLICATION_KIND src/ops`).

Test coverage that must stay unchanged by the extraction (all wire-level, through `MultiServiceRouter::dispatch`):
- `tests/all/service_error_rules.rs:1040-1106` — rows "an application no create minted, on a read" (`GetBenefitApplication`), "... on a submit", "... on an update", "... on a cancel" and "an application named by an ARN no create minted", each asserting 404 / `ResourceNotFoundException` and `message == not_found_message(APPLICATION_KIND, &identifier)`.
- `tests/all/amend_benefit_application.rs:282` and `:1124` — `not_found_message(APPLICATION_KIND, ...)` for `AmendBenefitApplication`.
- `tests/all/arn_identifier_fields.rs:123,293` — the same message for an ARN-spelled identifier.
- `tests/all/associate_benefit_application_resource.rs`, `tests/all/disassociate_benefit_application_resource.rs`, `tests/all/recall_benefit_application.rs` — each asserts the 404 path of its own operation.

## Steps to Reproduce

1. Read `src/state.rs:1587-1606`: the catalog lookup and its `ResourceNotFoundException` are one step, exposed once as `catalog()` / `catalog_refusal()`.
2. Read `src/state.rs:1629-1633` and `src/state.rs:1811-1818`: `application()` and `application_mut()` return a bare `Option` — no refusal constructor exists for the application resource.
3. Run `grep -n 'APPLICATION_KIND' services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops`: eight handler files each construct `ResourceNotFoundException { message: not_found_message(APPLICATION_KIND, <the request's identifier member>) }` themselves.
4. Compare `src/ops/get_benefit_application.rs:214-226` with `src/ops/associate_benefit_application_resource.rs:242-256`: the same rule, written twice, differing only in the error enum and the identifier member's name — so one copy can drift from the other seven.
5. Confirm the responses are pinned over the wire by `tests/all/service_error_rules.rs:1040-1106`, so hoisting the construction into `crate::state` leaves every response and every existing test unchanged.
