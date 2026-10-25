Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# The "NextToken this listing never issued" refusal is hand-rolled in all four list handlers instead of going through one helper, and the copies have already drifted

_Filed: 2026-10-24T22:59:51Z_
_Operation: ListExports | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this service turns `crate::state::page(...) -> None` into the modelled `ValidationException` by constructing the exception inline. The same rule — "a token this sequence never issued is the modelled 400, naming member `NextToken`, reason `fieldValidationFailed`" — is therefore written out four times, once per listing, and the four copies can drift independently.

They already have: two call sites take the reason from the crate's single constant `crate::state::VALIDATION_REASON_FIELD_FAILED`, and the other two hardcode the literal `"fieldValidationFailed"`, so a change to the constant would silently change two of the four refusals and not the other two.

The crate already has the pattern for exactly this shape of problem next door: the not-found refusal, which every ARN-addressed operation needs, is centralised in `crate::state::export_not_found_error(arn, wrap)` / `tagged_resource_not_found_error(arn, wrap)`, which take the calling operation's own error variant as a closure precisely "so every operation that looks an export ARN up reports the miss through this ONE place and none of them can drift". The page-token refusal is the same one-condition/many-operations case and does not follow it.

A fix is a `page`-side (or `state`-side) helper taking the `PageTokenKind`, the items, the paging members and the per-listing requirement sentence plus the operation's `ValidationException` wrapper — leaving every response byte and every existing test unchanged.

## Evidence

Four copies of the same branch and exception:

services/.../src/ops/list_exports.rs:179-196:
```
        let (exports, next_token) = crate::state::page(
            crate::state::PageTokenKind::Exports,
...
        .ok_or_else(|| {
            ListExportsErrorModeled::ValidationException(ValidationException {
                message: "NextToken is not a pagination token issued by this service."
                    .to_string(),
                reason: Some("fieldValidationFailed".to_string()),
```

services/.../src/ops/list_tags_for_resource.rs:214-220:
```
        .ok_or_else(
                || {
                    ListTagsForResourceErrorModeled::ValidationException(ValidationException {
                        message:
                            "NextToken is not a pagination token issued by this service."
                                .to_string(),
                        reason: Some("fieldValidationFailed".to_string()),
```

services/.../src/ops/list_tables.rs:159-163:
```
        .ok_or_else(|| {
            ListTablesErrorModeled::ValidationException(ValidationException {
                message: "NextToken is not a pagination token issued by this service."
                    .to_string(),
                reason: Some(crate::state::VALIDATION_REASON_FIELD_FAILED.to_string()),
```

services/.../src/ops/list_executions.rs:228-232:
```
        .ok_or_else(|| {
            ListExecutionsErrorModeled::ValidationException(ValidationException {
                message: "NextToken is not a pagination token issued by this service."
                    .to_string(),
                reason: Some(crate::state::VALIDATION_REASON_FIELD_FAILED.to_string()),
```
(the drift: two sites use the constant declared at services/.../src/state.rs:949 `pub const VALIDATION_REASON_FIELD_FAILED: &str = "fieldValidationFailed";`, two hardcode it.)

The crate's own pattern for the parallel case, services/.../src/state.rs:633-650:
```
/// `wrap` is the calling operation's own `ResourceNotFoundException` variant, so every
/// operation that looks an export ARN up reports the miss through this ONE place and none
/// of them can drift back to the shape's own status.
pub fn export_not_found_error<E>(
    arn: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
```

Tests that pin the behaviour this must not change: tests/all/export_lifecycle.rs:1000 (an unissued NextToken on ListExports must be refused) and tests/all/export_lifecycle.rs:1029 `a_page_token_is_not_accepted_by_the_other_listing`; tests/all/table_listing.rs:386 `an_unissued_next_token_is_refused`; tests/all/execution_listing.rs:404 `a_next_token_is_bound_to_the_executions_of_its_own_export`; tests/all/tagging_arn_validation.rs:180 (`"NextToken": "not-a-token-this-service-issued"`).

## Steps to Reproduce

1. Open src/ops/list_exports.rs, src/ops/list_tags_for_resource.rs, src/ops/list_tables.rs and src/ops/list_executions.rs and compare the `crate::state::page(...).ok_or_else(...)` blocks: the same branch and the same `ValidationException` shape appear four times.
2. Note that list_exports.rs:189 and list_tags_for_resource.rs:220 spell `"fieldValidationFailed"` as a literal while list_tables.rs:163 and list_executions.rs:232 read `crate::state::VALIDATION_REASON_FIELD_FAILED` — the copies have already drifted.
3. Compare with src/state.rs:636 `export_not_found_error` / :645 `tagged_resource_not_found_error`, the crate's single place for the parallel one-condition/many-operations refusal, which the page-token refusal does not use.
4. Run the crate's tests (export_lifecycle, table_listing, execution_listing, tagging_arn_validation) to confirm the existing refusals are pinned and must stay byte-identical after the de-duplication.
