Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# A BETWEEN study-date range whose upper bound names no DICOMStudyTime is refused as out of order, against the crate's own "a bound with no time bounds the whole date" rule

_Filed: 2026-10-29T10:45:10Z_
_Operation: SearchImageSets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RangeFilter::is_ordered` and `compare_study` give opposite meanings to an absent `DICOMStudyTime` on a bound.

`compare_study` — the function that actually evaluates the range — treats a bound with no `DICOMStudyTime` as bounding the WHOLE study date: it compares the dates only and answers `Ordering::Equal` for every study on that date, whatever its time. Its doc comment states exactly that rule.

`is_ordered`, the pre-flight check that decides whether the two bounds state a usable range, instead ranks each bound by `study_rank(date, time)`, which joins the date and the time with a control-character separator that (by its own doc) "sorts below every character either member can hold". An absent time therefore renders as the EMPTY string, i.e. the very START of that date rather than the whole of it.

So a request whose lower bound is `{DICOMStudyDate: D, DICOMStudyTime: "120000"}` and whose upper bound is `{DICOMStudyDate: D}` — a range the crate's own rule reads as "from D 12:00:00 to the end of D", and which `contains` would evaluate without trouble — is rejected before any state is read with `ValidationException` (400) "Member must hold the lower bound before the upper bound", because `"D\u{1f}120000" <= "D\u{1f}"` is false. The mirror image (`lower` date-only, `upper` with a time) passes and is then evaluated with the upper bound meaning the whole date, so the two directions of the same range disagree.

`DICOMStudyTime` is optional on `DICOMStudyDateAndTime` (only `DICOMStudyDate` is `@required`), so this is a bound combination the model admits, and the operation refuses it instead of searching.

## Evidence

The rule, stated by the crate — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/search_image_sets.rs:749-753`:
```
/// Where the study date and time an image set holds sits against `bound`, or `None` when
/// the image set holds no study date and so cannot be placed against one.
///
/// A bound that names no `DICOMStudyTime` bounds the whole study DATE: only the dates are
/// compared, so every study of that date is inside it whatever its time.
```
The same rule is restated in the module doc, `search_image_sets.rs:33-34`:
```
//! A study date and time bound that names no `DICOMStudyTime` bounds the whole study
//! DATE, which is what makes a one-day range expressible with the date alone.
```

The code that breaks it — `search_image_sets.rs:709-717`:
```
    fn is_ordered(&self) -> bool {
        match self {
            Self::CreatedAt { lower, upper } | Self::UpdatedAt { lower, upper } => lower <= upper,
            Self::StudyDateAndTime { lower, upper } => {
                study_rank(Some(&lower.dicom_study_date), lower.dicom_study_time.as_deref())
                    <= study_rank(Some(&upper.dicom_study_date), upper.dicom_study_time.as_deref())
            }
        }
    }
```
where an absent time becomes the empty string — `search_image_sets.rs:534-549`:
```
/// The separator between a study date and its time in a [`SortKey::Text`].
///
/// A control character, which sorts below every character either member can hold, so
/// comparing the joined text compares the date first and only then the time ...
const STUDY_RANK_SEPARATOR: char = '\u{1f}';
...
fn study_rank(study_date: Option<&str>, study_time: Option<&str>) -> String {
    format!(
        "{}{STUDY_RANK_SEPARATOR}{}",
        study_date.unwrap_or_default(),
        study_time.unwrap_or_default()
    )
}
```
and the refusal it drives — `search_image_sets.rs:604-610`:
```
                if !range.is_ordered() {
                    return Err(crate::state::handler_requirement_error(
                        &values_path,
                        None,
                        "Member must hold the lower bound before the upper bound",
                    ));
                }
```

The evaluator the refusal pre-empts — `search_image_sets.rs:729-738` and `754-768` — places any study of the bound's date inside a time-less bound:
```
    let Some(bound_time) = bound.dicom_study_time.as_deref() else {
        return Some(Ordering::Equal);
    };
```

The model admits the combination: `smithy_query(query="shape",shape="DICOMStudyDateAndTime")` gives `DICOMStudyDate` `"required": true` and `DICOMStudyTime` with no `required` flag.

Test coverage: `tests/all/image_set_search.rs:434-443` ("BETWEEN over study dates, each bound a whole date" — both bounds date-only) and `:445-460` ("BETWEEN over one day's study times" — both bounds carrying a time) are the only study-date BETWEEN success rows, and `:935` ("BETWEEN over study dates with the bounds the wrong way round") is the only ordering refusal row. The MIXED case (one bound with a time, the other date-only) is exercised by no row, so no existing test pins the behaviour reported here.

## Steps to Reproduce

1. CreateDatastore, wait for ACTIVE, and get an image set into it holding DICOMStudyDate "19850412" and DICOMStudyTime "150000.000000"
2. POST /datastore/{datastoreId}/searchImageSets with searchCriteria.filters = [{"operator":"BETWEEN","values":[{"DICOMStudyDateAndTime":{"DICOMStudyDate":"19850412","DICOMStudyTime":"120000.000000"}},{"DICOMStudyDateAndTime":{"DICOMStudyDate":"19850412"}}]}]
3. Observe 400 ValidationException "1 validation error detected: searchCriteria.filters[0].values failed to satisfy constraint: Member must hold the lower bound before the upper bound" — the range "from 19850412 12:00:00 to the end of 19850412" is never evaluated
4. Swap the two bounds (date-only first, timed second) and observe the request is accepted, with the date-only bound then read as the whole date — the two directions of the same range disagree
