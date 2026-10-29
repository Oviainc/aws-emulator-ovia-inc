Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# SearchImageSets hand-spells the enum-value refusal clause instead of raising ViolationKind::EnumValue, so the same violation class reads two different ways in one service

_Filed: 2026-10-29T17:48:35Z_
_Operation: SearchImageSets | Class: error-response_

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has exactly one rule for how a handler words a constraint violation it raises itself: route it through a `smithy_server_runtime::Violation` and let `crate::state::one_validation_error` render the framework's canonical clause, so that "a refusal raised past the boundary validator reads exactly like one the boundary stated". Two of the three handlers that re-check a modelled enum do that (`ListDatastores` for `datastoreStatus`, `ListDICOMImportJobs` for `jobStatus`): they build `ViolationKind::EnumValue(<the generated *_VALUES constant>)`, so the requirement half of the clause is the framework's own text and the advertised value set is the model's own list.

`SearchImageSets` does neither. Its three enum re-checks (`searchCriteria.sort.sortField`, `searchCriteria.sort.sortOrder`, and `searchCriteria.filters.<N>.member.operator`) go through `crate::state::handler_requirement_error` — the seam whose own doc says it is for "a requirement no [`ViolationKind`] states" — with the requirement text typed out by hand. Two things diverge as a result:

1. The framework spells the requirement `Member must satisfy enum value set: [...]` (with a colon after `set`); the three hand-written strings spell it `Member must satisfy enum value set [...]` with no colon. So this one service answers the SAME violation class with two different sentences depending on which operation was called.
2. The value list is a hard-coded literal where the crate already carries the model-derived constant (`SORT_FIELD_VALUES`, `SORT_ORDER_VALUES`, `OPERATOR_VALUES` in `ops/types/shared_types.rs`). The hand-written lists are also in a different order from those constants (`[updatedAt, createdAt, DICOMStudyDateAndTime]` vs `["DICOMStudyDateAndTime", "createdAt", "updatedAt"]`; `[EQUAL, BETWEEN]` vs `["BETWEEN", "EQUAL"]`), so a client reconstructing the advertised value set from the message gets a different answer from `SearchImageSets` than from `ListDatastores`, and a model change to any of the three enums silently stops being reflected here.

These are the service's own defensive branches — the handler comments state they exist for "a caller got past" the boundary validator — so the defect is in the wording the branch emits, not in whether the branch should exist. No behavioural state is affected, hence LOW; the fix is to raise the violation the crate already has a kind for, exactly as the two sibling listings do.

## Evidence

The canonical requirement text the framework renders for this violation class, with the colon — vendor/smithy-schema-server/src/constraints.rs:151-156:
```
            ViolationKind::EnumValue(allowed) => {
                format!(
                    "Member must satisfy enum value set: [{}]",
                    allowed.join(", ")
                )
            }
```

The crate's own rule that this rendering is REUSED — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:438-443:
```
/// One constraint violation, worded the way this service words it.
///
/// The framework already states every violation class in the AWS sentence
/// (`Value '<v>' at '<path>' failed to satisfy constraint: <requirement>`), so that
/// rendering is REUSED — a constraint class this service has never seen is still phrased
/// like the rest — and exactly two things are changed:
```
and state.rs:572-575:
```
/// The envelope for a single violation a HANDLER found itself, so a refusal raised past
/// the boundary validator reads exactly like one the boundary stated. Delegates to
/// [`validation_envelope`], so the envelope text is spelled in one place.
pub fn one_validation_error(violation: &Violation) -> String {
```
and the seam `SearchImageSets` uses instead, whose doc scopes it to the opposite case — state.rs:579-581:
```
/// The `ValidationException` message for a requirement no [`ViolationKind`] states —
/// one a HANDLER checks itself, such as a member the model documents as holding a JSON
/// document, or two members that cannot hold together.
```

The sibling that DOES raise the violation kind — src/ops/list_datastores.rs:148-174:
```
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
                                },
                            ),
                        },
                    )
                    .into());
                }
```
and the identical shape in src/ops/list_dicom_import_jobs.rs:159-174 (`JOB_STATUS_VALUES`, `ViolationKind::EnumValue`).

The three hand-spelled clauses in `SearchImageSets` — src/ops/search_image_sets.rs:417-430:
```
        let field = SortField::from_wire(&sort.sort_field).ok_or_else(|| {
            crate::state::handler_requirement_error(
                "searchCriteria.sort.sortField",
                Some(&sort.sort_field),
                "Member must satisfy enum value set [updatedAt, createdAt, DICOMStudyDateAndTime]",
            )
        })?;
        let order = SortOrder::from_wire(&sort.sort_order).ok_or_else(|| {
            crate::state::handler_requirement_error(
                "searchCriteria.sort.sortOrder",
                Some(&sort.sort_order),
                "Member must satisfy enum value set [ASC, DESC]",
            )
        })?;
```
and src/ops/search_image_sets.rs:620-624:
```
            other => Err(crate::state::handler_requirement_error(
                &format!("{element_path}.operator"),
                Some(other),
                "Member must satisfy enum value set [EQUAL, BETWEEN]",
            )),
```

The model-derived constants the crate already owns and these three literals bypass — src/ops/types/shared_types.rs:103-112:
```
/// The values `Operator` accepts on the wire.
pub const OPERATOR_VALUES: &[&str] = &["BETWEEN", "EQUAL"];
/// The values `SortField` accepts on the wire.
pub const SORT_FIELD_VALUES: &[&str] = &["DICOMStudyDateAndTime", "createdAt", "updatedAt"];
/// The values `SortOrder` accepts on the wire.
pub const SORT_ORDER_VALUES: &[&str] = &["ASC", "DESC"];
```
The handlers' own statement that these defensive branches are meant to be reachable — src/ops/search_image_sets.rs:406-409: "A value outside either enum is refused rather than silently defaulted: the boundary validator already states the enum, so reaching here with another value means a caller got past it"; src/ops/list_datastores.rs:145-147: "The boundary validator already refused anything else, so an unparseable value here can only come from a caller bypassing it."

Test coverage: `grep -r "enum value set" services/service-com-amazonaws-medicalimaging-ahigatewayservice` matches ONLY src/ops/search_image_sets.rs:421,428,623 — no test anywhere pins the colon-less wording or the hand-written value order, so nothing justifies the divergence.

## Steps to Reproduce

1. Read src/ops/list_datastores.rs:158-169 and src/ops/list_dicom_import_jobs.rs:162-174: both build a `smithy_server_runtime::Violation` whose `kind` is `ViolationKind::EnumValue(<model-derived *_VALUES>)` and render it with `crate::state::one_validation_error`.
2. Read vendor/smithy-schema-server/src/constraints.rs:151-156: that kind renders the requirement as `Member must satisfy enum value set: [ACTIVE, CREATE_FAILED, CREATING, DELETED, DELETING]` — with a colon after `set` and the model's own value order.
3. Read src/ops/search_image_sets.rs:417-430 and 620-624: the same violation class is instead passed to `crate::state::handler_requirement_error` with the requirement typed out as `Member must satisfy enum value set [updatedAt, createdAt, DICOMStudyDateAndTime]` / `[ASC, DESC]` / `[EQUAL, BETWEEN]` — no colon, and a list order that matches none of `SORT_FIELD_VALUES`, `SORT_ORDER_VALUES`, `OPERATOR_VALUES` in src/ops/types/shared_types.rs:103-112.
4. Compare the two clauses a client would read for the one violation class: `... failed to satisfy constraint: Member must satisfy enum value set: [...]` from `ListDatastores`/`ListDICOMImportJobs` versus `... failed to satisfy constraint: Member must satisfy enum value set [...]` from `SearchImageSets`.
5. Expected (state.rs:572-574 and state.rs:438-443): one wording for one violation class, taken from the framework's rendering, with the value set read from the generated constant.
6. Actual: `SearchImageSets` emits its own sentence and its own hand-copied value list, which a model change to `SortField`, `SortOrder` or `Operator` would not update.
