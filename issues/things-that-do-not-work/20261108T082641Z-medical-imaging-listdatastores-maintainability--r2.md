Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4794fe8886d36994e3b764249716f059e206492d

# The "re-check a modelled enum query filter" refusal is implemented twice, once per paginated listing, instead of in the state module that owns this service's refusal wording

_Filed: 2026-11-08T08:26:41Z_
_Operation: ListDatastores | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4794fe8886d36994e3b764249716f059e206492d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has one rule for refusing a `@httpQuery` status filter whose value names no modelled enum member: parse the wire value with the enum's `from_wire`, and on failure build a `smithy_server_runtime::Violation` whose `kind` is `ViolationKind::EnumValue(<the generated *_VALUES constant>)` and render it through `crate::state::one_validation_error`, so the clause reads exactly like one the boundary validator stated.

That rule is written out TWICE, character-for-character apart from the enum type, the generated values constant and the member path: once in `ListDatastores` for `datastoreStatus` and once in `ListDICOMImportJobs` for `jobStatus`. Every other cross-listing rule in this crate has already been lifted into `crate::state` for exactly this reason — the page-size clamp (`crate::state::list_page_size`, whose doc says "Lives here, like the token codec above, because every paginated listing shares it and a page-size rule spelled once per listing drifts: one clamp updated, the others left behind"), the page-token codec (`crate::state::encode_page_token` / `decode_page_token`), the collection-filter marker (`crate::state::page_token_filter_segment`), the tag-map rules (`crate::state::tag_map_refusal`) and the quota wordings (`crate::state::extra_attributes_quota_message`, `crate::state::too_many_tags_message`). This one rule stayed in the handlers.

The copies have already drifted in practice: a THIRD site in this crate re-checks a modelled enum (`SearchImageSets` for `searchCriteria.sort.sortField`, `searchCriteria.sort.sortOrder` and `searchCriteria.filters.<N>.member.operator`) and words the identical violation class a different way — it calls `crate::state::handler_requirement_error` with the requirement sentence typed out by hand and the value list hard-coded, which is the subject of the open issue `20261029T174835Z-medical-imaging-searchimagesets-error-response--r2.md`. That issue's prescribed fix is to make `SearchImageSets` build the same `ViolationKind::EnumValue` violation the two listings build, i.e. to add a third copy of this block. A single `crate::state` helper taking (path, rejected value, allowed values) gives all three one implementation and makes a future enum re-check impossible to spell differently.

No response changes: the helper would build the same `Violation` and call the same `one_validation_error`, so both listings emit byte-identical messages, the same `ValidationException` code and the same HTTP 400.

## Evidence

Copy 1 — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/list_datastores.rs:147-178:
```
        // The status filter must name a modelled `DatastoreStatus`. The boundary
        // validator already refused anything else, so an unparseable value here can
        // only come from a caller bypassing it.
        let status_filter = match input.datastore_status.as_deref() {
            None => None,
            Some(value) => match DatastoreStatus::from_wire(value) {
                Some(status) => Some(status),
                None => {
                    return Err(ListDatastoresErrorModeled::ValidationException(
                        ValidationException {
                            // Worded through the one refusal formatter this service
                            // owns, so a refusal raised here reads exactly like one
                            // the boundary validator stated.
                            message: crate::state::one_validation_error(
                                &smithy_server_runtime::Violation {
                                    path: "datastoreStatus".to_string(),
                                    kind: smithy_server_runtime::ViolationKind::EnumValue(
                                        DATASTORE_STATUS_VALUES
                                            .iter()
                                            .map(|value| (*value).to_string())
                                            .collect(),
                                    ),
                                    value: Some(value.to_string()),
                                    sensitive: false,
                                },
                            ),
                        },
                    )
                    .into());
                }
            },
        };
```

Copy 2 — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/list_dicom_import_jobs.rs:150-181, the same block with `JobStatus` / `JOB_STATUS_VALUES` / `"jobStatus"` substituted:
```
        // The status filter must name a modelled `JobStatus`. The boundary validator
        // already refused anything else, so an unparseable value here can only come from
        // a caller bypassing it.
        let status_filter = match input.job_status.as_deref() {
            None => None,
            Some(value) => match JobStatus::from_wire(value) {
                Some(status) => Some(status),
                None => {
                    return Err(ListDICOMImportJobsErrorModeled::ValidationException(
                        ValidationException {
                            // Worded through the one refusal formatter this service
                            // owns, so a refusal raised here reads exactly like one the
                            // boundary validator stated.
                            message: crate::state::one_validation_error(
                                &smithy_server_runtime::Violation {
                                    path: "jobStatus".to_string(),
                                    kind: smithy_server_runtime::ViolationKind::EnumValue(
                                        JOB_STATUS_VALUES
                                            .iter()
                                            .map(|value| (*value).to_string())
                                            .collect(),
                                    ),
                                    value: Some(value.to_string()),
                                    sensitive: false,
                                },
                            ),
                        },
                    )
                    .into());
                }
            },
        };
```

The crate's own precedent for lifting a shared listing rule out of the handlers — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:2407-2415:
```
/// How many records one page of a listing holds, from the `maxResults` the caller named.
///
/// Lives here, like the token codec above, because every paginated listing shares it and
/// a page-size rule spelled once per listing drifts: one clamp updated, the others left
/// behind.
```
and both handlers already defer to it — list_datastores.rs:194-196 and list_dicom_import_jobs.rs:199-201:
```
        // The page-size rule is the service's one ([`crate::state::list_page_size`]), so
        // every listing bounds a page the same way.
        let page_size = crate::state::list_page_size(input.max_results);
```

Proof the copies can drift: the third enum re-check in this crate words the same class differently — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/search_image_sets.rs:424-437:
```
        let field = SortField::from_wire(&sort.sort_field).ok_or_else(|| {
            crate::state::handler_requirement_error(
                "searchCriteria.sort.sortField",
                Some(&sort.sort_field),
                "Member must satisfy enum value set [updatedAt, createdAt, DICOMStudyDateAndTime]",
            )
        })?;
```

Test coverage that must stay green and does, because no response changes: tests/all/import_job_listing.rs:433-442 drives the refusal over real HTTP and asserts only the status and the error code —
```
    // A value no modelled `JobStatus` names is a client error, not an empty page.
    let refused = list_jobs(&emulator, &datastore_id, "jobStatus=NOT_A_STATUS").await;
    assert_eq!(
        refused.status, 400,
        "case \"unmodelled jobStatus\": expected HTTP 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_code, "ValidationException",
        "case \"unmodelled jobStatus\": expected ValidationException, got {refused:?}"
    );
```
The accepted-filter behaviour of both listings is pinned by tests/all/listing_filter_page_tokens.rs:154 (`&datastoreStatus={status}` over `DATASTORE_STATUS_VALUES`) and :348 (`&jobStatus={status}` over `JOB_STATUS_VALUES`), plus tests/all/datastore_deleted_window.rs:109-112 and tests/all/import_job_listing.rs:387-431 — none of which reads the refusal message text, so extracting the helper leaves every one of them unchanged.

## Steps to Reproduce

1. Read services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/list_datastores.rs:147-178 — the parse-or-refuse block for the `datastoreStatus` query filter.
2. Read services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/list_dicom_import_jobs.rs:150-181 — the identical block for `jobStatus`, differing only in the enum type (`JobStatus`), the generated constant (`JOB_STATUS_VALUES`) and the violation path string.
3. Diff the two: the comment, the nested `match`, the `ViolationKind::EnumValue(<constant>.iter().map(...).collect())` construction, the `value` / `sensitive` fields and the `one_validation_error` call are the same rule written twice.
4. Read services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:2407-2415 and the two call sites of `crate::state::list_page_size` (list_datastores.rs:194-196, list_dicom_import_jobs.rs:199-201): every other rule shared by these two listings already lives in the state module precisely because "a page-size rule spelled once per listing drifts".
5. Read services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/search_image_sets.rs:424-437 and 620-624: a third enum re-check in the same crate spells the same violation class by hand instead, which is the drift this duplication permits (open issue 20261029T174835Z-medical-imaging-searchimagesets-error-response--r2.md).
6. Expected: one implementation of "refuse a query value that names no modelled enum member" in `crate::state`, called by every listing that re-checks an enum. Actual: two byte-equivalent copies in the two listing handlers, with a third site already wording it differently.
