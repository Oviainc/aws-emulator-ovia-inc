Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Accept the Filter.Operation spellings the model documents (eq/geq/leq) instead of refusing them

_Filed: 2026-10-24T13:26:29Z_
_Operation: ListProductSubscriptions | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Filter.Operation` is documented as "The type of search (For example, eq, geq, leq)", but the shared filter parser accepts only `equals`/`eq`, `notequals`/`ne`, `contains`, `beginswith` (case-insensitively) and refuses everything else with HTTP 400 ValidationException "Filter.Operation <op> is not supported".

Expected: a filter using a spelling the model's own documentation gives as an example (`geq`, `leq`) is honoured as the corresponding comparison rather than rejected.

Actual: `parse_filter` rejects `geq` and `leq`, so a client that follows the documented example gets a 400 from ListProductSubscriptions (and, through the same helper, from ListIdentityProviders, ListInstances and ListLicenseServerEndpoints). Fixing the shared helper in `src/state.rs` fixes all four list operations at once.

## Evidence

Spec: `Filter$Operation` documentation (smithy_query doc, shape Filter, member Operation) — "The type of search (For example, eq, geq, leq)". Code: `src/state.rs` `parse_filter` (~lines 2122-2134) matches only equals/eq, notequals/ne, contains, beginswith and otherwise returns `StateError::Validation("Filter.Operation .. is not supported")`, which each list handler maps onto its modelled ValidationException (e.g. `src/ops/list_product_subscriptions.rs` calls `validate_filters` before reading state). Tests: `tests/all/identity_provider_subscriptions.rs::filters_select_on_the_documented_attributes` and `tests/all/list_instances.rs::a_filter_the_operation_does_not_expose_is_refused` cover attributes and unknown operations; none exercises the documented geq/leq spellings.

## Steps to Reproduce

1. RegisterIdentityProvider for a usable DirectoryId and Product, then StartProductSubscription for a user.
2. Call ListProductSubscriptions with that IdentityProvider and Filters=[{Attribute: "Username", Operation: "geq", Value: "a"}].
3. Expected: HTTP 200 with the filtered page (the operation spelling is one the model documents). Actual: HTTP 400 ValidationException "Filter.Operation geq is not supported; supported operations: Equals, NotEquals, Contains, BeginsWith".
4. Repeat against ListIdentityProviders / ListInstances / ListLicenseServerEndpoints with Operation="leq" and observe the same refusal from the shared helper.
