Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# The two-stage "resolve the report version a request names" rule is written out three times, once per report-reading handler

_Filed: 2026-10-29T04:14:14Z_
_Operation: GetReportMetadata | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers of this crate resolve the report a request targets in exactly the same two stages, and each writes the whole rule out itself:

1. an id the catalog does not hold is NOT the modelled 404 — it is the front door's `Scaffolded::unauthorized()` 401, because the catalog is the account's entitlement; then
2. a report that IS held but publishes no such version (or, when the request named no version, publishes none at all) is the modelled `ResourceNotFoundException`, whose payload is built by the shared `crate::state::report_version_not_found(&input.report_id, input.report_version)`.

`get_report_metadata.rs`, `get_report.rs` and `get_term_for_report.rs` carry byte-for-byte identical copies of both stages — the only thing that differs between them is which operation's error enum the `ResourceNotFoundException` is lifted into. `list_report_versions.rs` carries a fourth copy of stage 1 alone.

The copies can drift: a change to which of the two refusals a given case gets (the very distinction `state.rs:2292-2296` spells out, and the subject of the open issue `20261028T222329Z-artifact-getreport-error-response`) has to be made in three places or the three report readers start disagreeing about the same `reportId`/`reportVersion` pair.

This crate already hoists exactly this kind of shared judgement, and it already has the idiom for the "each operation lifts it into its own modelled error variant" part: `crate::state::resume_position` decides the pagination-token rule once and hands the refusal back for each listing to lift (`.map_err(ListReportsErrorModeled::ValidationException)?`). A `crate::state` helper that resolves a report version and hands back either the version or which of the two refusals applies would collapse the three copies the same way, leaving every status, body and header exactly as it is.

## Evidence

Copy A — `services/service-com-amazonaws-artifact-artifact/src/ops/get_report_metadata.rs:174-189`:
```
let Some(report) = store.reports.get(&input.report_id) else {
    // Authorization of the targeted report precedes the catalog lookup as far as
    // a caller can tell: an id the catalog does not publish names no report this
    // account may read, so the front door's refusal is what it sees.
    return Err(crate::scaffolded::Scaffolded::unauthorized());
};
let Some(version) = report.resolve(input.report_version) else {
    ...
    let missing =
        crate::state::report_version_not_found(&input.report_id, input.report_version);
    return Err(GetReportMetadataErrorModeled::ResourceNotFoundException(missing).into());
};
```

Copy B — `services/service-com-amazonaws-artifact-artifact/src/ops/get_report.rs:189-200`:
```
let Some(report) = store.reports.get(&input.report_id) else {
    // Authorization of the targeted report precedes the catalog lookup as far as
    // a caller can tell: an id the catalog does not publish names no report this
    // account may read, so the front door's refusal is what it sees.
    return Err(crate::scaffolded::Scaffolded::unauthorized());
};
let Some(version) = report.resolve(input.report_version) else {
    // The report is reachable; this version of it does not exist.
    let missing =
        crate::state::report_version_not_found(&input.report_id, input.report_version);
    return Err(GetReportErrorModeled::ResourceNotFoundException(missing).into());
};
```

Copy C — `services/service-com-amazonaws-artifact-artifact/src/ops/get_term_for_report.rs:189-200`:
```
let Some(report) = store.reports.get(&input.report_id) else {
    // Authorization of the targeted report precedes the catalog lookup as far as
    // a caller can tell: an id the catalog does not publish names no report this
    // account may read, so the front door's refusal is what it sees.
    return Err(crate::scaffolded::Scaffolded::unauthorized());
};
let Some(version) = report.resolve(input.report_version) else {
    // The report is reachable; this version of it does not exist.
    let missing =
        crate::state::report_version_not_found(&input.report_id, input.report_version);
    return Err(GetTermForReportErrorModeled::ResourceNotFoundException(missing).into());
};
```

Stage 1 alone, a fourth time — `services/service-com-amazonaws-artifact-artifact/src/ops/list_report_versions.rs:187-192`:
```
let Some(report) = store.reports.get(&input.report_id) else {
    // Authorization of the targeted report precedes the catalog lookup as far as
    // a caller can tell: an id the catalog does not publish names no report this
    // account may read, so the front door's refusal is what it sees.
    return Err(crate::scaffolded::Scaffolded::unauthorized());
};
```

The crate states that this is ONE rule rather than four, in the state module that owns the payload — `services/service-com-amazonaws-artifact-artifact/src/state.rs:2291-2296`:
```
/// The `ResourceNotFoundException` payload for a version the named report never had.
///
/// A report the catalog does not hold is NOT this error: the front door refuses that
/// call before the catalog is consulted (see [`crate::scaffolded::Scaffolded`]). This
/// error is for a report that IS visible and simply never published the version asked
/// for — or, when the request named no version, never published any at all.
```

The shape the hoist should take already exists in this crate for the same "one rule, each operation lifts it into its own error variant" problem — `services/service-com-amazonaws-artifact-artifact/src/state.rs:2638-2648`:
```
/// The refusal is handed back for the caller to lift into its OWN modelled
/// `ValidationException` variant — the only part of this decision that differs between
/// the paginated operations.
pub fn resume_position(
    next_token: Option<&str>,
    parse: impl FnOnce(&str) -> Option<usize>,
) -> Result<usize, ValidationException> {
```

Existing coverage that must stay green and unchanged (both halves of the rule are pinned on every report operation):
- `tests/all/report_workflow.rs:712-751` — `a_report_id_naming_nothing_is_unauthorized_on_every_report_operation`, asserting 401 `UnauthorizedException` for `GetReportMetadata`, `ListReportVersions`, `GetTermForReport` and `GetReport`;
- `tests/all/report_workflow.rs:436-452` — "One past the newest version does not exist", asserting 404 `ResourceNotFoundException`;
- `tests/all/front_door_errors.rs:211` — `a_report_target_the_front_door_cannot_authorize_is_refused_with_401`;
- `tests/all/validation_exception_shape.rs:390` — the `(404, "ResourceNotFoundException")` envelope row.

## Steps to Reproduce

1. Open services/service-com-amazonaws-artifact-artifact/src/ops/get_report_metadata.rs and read lines 174-189.
2. Open src/ops/get_report.rs lines 189-200 and src/ops/get_term_for_report.rs lines 189-200: the same two `let ... else` blocks, with the same comments, differing only in the error enum the `ResourceNotFoundException` is wrapped in.
3. Open src/ops/list_report_versions.rs lines 187-192 for a fourth copy of the first stage.
4. Read src/state.rs:2291-2296, where the crate states the two-stage rule once as the contract behind `report_version_not_found`.
5. Compare with src/state.rs:2638-2648 (`resume_position`), the crate's existing idiom for deciding one rule in `state` and letting each operation lift the refusal into its own modelled error variant.
6. Note that collapsing the four copies into one `state` helper changes no status, body or header: tests/all/report_workflow.rs:712-751 and :436-452, tests/all/front_door_errors.rs:211 and tests/all/validation_exception_shape.rs:390 pin both refusals on every report operation and must pass unmodified.
