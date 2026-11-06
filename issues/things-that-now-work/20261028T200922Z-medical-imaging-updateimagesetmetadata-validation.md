Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3ebf0e2f135a02ae75ffd55898f9ee21fc25614a

# Modelled DICOMTags members are written from the update document without their modelled range/length/pattern, so SearchImageSets reports values the model forbids

_Filed: 2026-10-28T20:09:22Z_
_Operation: UpdateImageSetMetadata | Class: validation_

> _Note: this issue was identified against the code base at commit `3ebf0e2f135a02ae75ffd55898f9ee21fc25614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateImageSetMetadata` routes an attribute the model NAMES into the corresponding `DICOMTags` member, gated only by `ModelledAttribute::admits`, which checks the JSON *type* and nothing else: an integer member admits any value that fits in `i32`, and a text member admits any JSON string whatever its length or shape. None of the constraints the model puts on those members is applied, so a document can persist a value the member cannot legally carry, and `SearchImageSets` then reports it inside `DICOMTags` and `GetImageSetMetadata` inside the served document.

Concretely, through `updatableAttributes` (an opaque blob the boundary validator does not walk):

* `Study.DICOM.NumberOfStudyRelatedSeries` / `NumberOfStudyRelatedInstances` accept `-5` or `2000000000`, although the model declares `@range(min: 0, max: 1000000)` on `DICOMNumberOfStudyRelatedSeries` and `DICOMNumberOfStudyRelatedInstances`. The generated member is a plain `i32` (`src/ops/types/search_image_sets_types.rs:97`), so the out-of-range number is stored and reported verbatim.
* `Study.DICOM.StudyInstanceUID` accepts `"NOT-A-UID"`, although `DICOMStudyInstanceUID` declares `@pattern("^[0-9.]+$")`; the same goes for `SeriesInstanceUID`.
* `Patient.DICOM.PatientSex` accepts a 1000-character string, although `DICOMPatientSex` declares `@length(min: 0, max: 16)`.

The handler's own documentation asserts the opposite rule — `admits` is documented as deciding whether the value is "an integer the modelled member can carry" — so nothing in the crate justifies the divergence; the comment states the rule the code does not implement. The result is an image set whose persisted state cannot be rendered within the model's own constraints, reachable from a plain 200 response.

## Evidence

The only gate on a modelled attribute's value, with the doc stating the rule it does not enforce — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/update_image_set_metadata.rs:276-285`:
```
    /// Whether `value` is of the kind this attribute holds — an integer the modelled
    /// member can carry, or text.
    fn admits(self, value: &Value) -> bool {
        match self.kind() {
            AttributeKind::Integer => value
                .as_i64()
                .is_some_and(|number| i32::try_from(number).is_ok()),
            AttributeKind::Text => value.is_string(),
        }
    }
```
and the apply path, which refuses only on that type check before writing — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/update_image_set_metadata.rs:694-707`:
```
            Some(modelled) => {
                if !modelled.admits(&value) {
                    ...
                            "Member must hold {} for {keyword}",
                    ...
                }
                modelled.write(&mut attributes.dicom_tags, Some(&value));
```
with `write` truncating to nothing more than an `i32` — `:328-332`:
```
        let number = || {
            value
                .and_then(Value::as_i64)
                .and_then(|number| i32::try_from(number).ok())
        };
```

The model's constraints on the very members being written (`smithy_query(query="shape", shape="DICOMTags")`):
```
"DICOMNumberOfStudyRelatedSeries": {"default": 0, "range": {"min": 0, "max": 1000000}, "type": "integer", "doc": "<p>The total number of series in the DICOM study.</p>"}
"DICOMNumberOfStudyRelatedInstances": {"default": 0, "range": {"min": 0, "max": 1000000}, "type": "integer"}
"DICOMStudyInstanceUID": {"length": {"min": 0, "max": 512}, "pattern": "^[0-9.]+$", "sensitive": true}
"DICOMPatientSex": {"length": {"min": 0, "max": 16}, "sensitive": true}
```

The stored value is then reported back inside that same constrained shape — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/search_image_sets.rs:332`:
```
        dicom_tags: Some(image_set.dicom_tags.clone()),
```
and `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/get_image_set_metadata.rs:170-177`:
```
    dicom.insert(
        "NumberOfStudyRelatedSeries".to_string(),
        Value::from(tags.dicom_number_of_study_related_series),
    );
```

Test coverage: `tests/all/image_set_metadata_updates.rs:745` pins only the TYPE refusal (`"NumberOfStudyRelatedSeries": "four"`), and lines 276-291 / 388-395 pin in-range values (4, 17, 1). No test pins acceptance of an out-of-range number, an over-long string or a non-numeric UID, so enforcing the modelled constraints changes no existing assertion.

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE; obtain an ACTIVE image set in it (as `tests/all/image_set_metadata_updates.rs` seeds one).
2. POST `/datastore/{datastoreId}/imageSet/{imageSetId}/updateImageSetMetadata?latestVersion=1` with body {"DICOMUpdates":{"updatableAttributes":"{\"Study\":{\"DICOM\":{\"NumberOfStudyRelatedSeries\":-5,\"StudyInstanceUID\":\"NOT-A-UID\"}},\"Patient\":{\"DICOM\":{\"PatientSex\":\"<1000 characters>\"}}}"}}.
3. Observed: HTTP 200, a new version is minted. Expected: ValidationException (400), since the model declares @range(0..1000000) on DICOMNumberOfStudyRelatedSeries, @pattern(^[0-9.]+$) on DICOMStudyInstanceUID and @length(0..16) on DICOMPatientSex.
4. Wait for IMAGE_SET_UPDATE_DELAY, then POST SearchImageSets on the data store with no criteria.
5. Observed: the summary's `DICOMTags` carries `DICOMNumberOfStudyRelatedSeries: -5`, `DICOMStudyInstanceUID: "NOT-A-UID"` and a 1000-character `DICOMPatientSex` — all outside the constraints the model declares for those members.
6. POST GetImageSetMetadata on the same image set: the served document reports the same illegal values.
