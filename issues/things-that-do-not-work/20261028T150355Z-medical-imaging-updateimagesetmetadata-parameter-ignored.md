Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# Series-level attributes in updatableAttributes are dropped when the image set holds no SeriesInstanceUID: two series collapse into one and the stored value becomes unreportable

_Filed: 2026-10-28T15:03:55Z_
_Operation: UpdateImageSetMetadata | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`apply_document` decides whether an attribute is one the model NAMES by calling `modelled_attribute(&at, series_instance_uid)`, whose series arm matches when `series_instance_uid.is_none_or(|held| held == *series)`. For an image set that holds no `dicomSeriesInstanceUID` yet, `is_none_or` makes EVERY `Study → Series → <any uid> → DICOM → <keyword>` path count as the one modelled series. Two consequences, both silent:

1. A document naming the same series-level keyword under TWO different series UIDs writes both into the single modelled member and `remove_at`s both from `extra_attributes`. The first series' value is neither in `dicom_tags` (overwritten by the second) nor kept at the path it was written at — it is gone, while the call answers 200.
2. Even with one series named, the value lands in `dicom_tags.dicom_series_modality` (etc.) and is removed from `extra_attributes`, but `GetImageSetMetadata::series` emits a series level ONLY when `dicom_series_instance_uid` is `Some`. So the attribute the caller wrote is reported by `SearchImageSets` (which serves `dicom_tags`) and is absent from the metadata document — exactly the disagreement this handler's own module doc says cannot happen.

The same disagreement is reachable on a fully populated image set: a forced removal of `Study.Series.<uid>.DICOM.SeriesInstanceUID` clears `dicom_series_instance_uid` while leaving `dicom_series_modality` / `dicom_series_body_part` / `dicom_series_number` stored, after which the document reports no series at all and those stored attributes are unreachable through `GetImageSetMetadata`.

## Evidence

The invariant the handler states for itself — `src/ops/update_image_set_metadata.rs:29-33`:
```
//! * an attribute the model NAMES (`DICOMTags`, sixteen members) is written to that
//!   member, so what `SearchImageSets` filters on and what the metadata document reports
//!   never disagree. Every OTHER attribute — DICOM defines thousands — is persisted at the
//!   path it was written at ([`crate::state::ImageSetAttributes::extra_attributes`]) and
//!   reported from there, rather than accepted and dropped.
```
The match that breaks it — `src/ops/update_image_set_metadata.rs:322-324, 347-357`:
```
/// `series_instance_uid` is the series the image set holds: the model declares the
/// attributes of ONE series per image set, so a path naming another series describes
/// content the modelled members cannot hold and is kept as written.
...
        [STUDY_KEY, SERIES_KEY, series, DICOM_KEY, _]
            if series_instance_uid.is_none_or(|held| held == *series) =>
        {
            match keyword {
                "SeriesInstanceUID" => Some(ModelledAttribute::SeriesInstanceUid),
                "Modality" => Some(ModelledAttribute::SeriesModality),
```
and the write that also erases the written path — `src/ops/update_image_set_metadata.rs:520-546`:
```
    for DocumentAttribute { path: at, value } in attributes_of(document) {
        let series_instance_uid = attributes.dicom_tags.dicom_series_instance_uid.clone();
        match modelled_attribute(&at, series_instance_uid.as_deref()) {
...
            Some(modelled) => {
...
                modelled.write(&mut attributes.dicom_tags, Some(&value));
                remove_at(&mut attributes.extra_attributes, &at);
            }
            None if removing => remove_at(&mut attributes.extra_attributes, &at),
            None => write_at(&mut attributes.extra_attributes, &at, value),
```
The reader that cannot report it, with its own stated reason — `src/ops/get_image_set_metadata.rs:217-249`:
```
/// This service persists the attributes of ONE series per image set (the model's
/// `DICOMTags` declares one `dicomSeriesInstanceUID`), so an image set that has had DICOM
/// content delivered to it reports that series and an image set nothing has delivered
/// content to reports none — rather than a series with no identifier, which no caller
/// could read an instance out of.
fn series(image_set: &ImageSet) -> Value {
    let tags = &image_set.dicom_tags;
    let mut series = Map::new();
    if let Some(series_instance_uid) = tags.dicom_series_instance_uid.as_ref() {
```
No test pins this: every row of `update_cases()` runs against an image set that already holds the one seeded series — `tests/all/image_set_metadata_updates.rs:50-51` ("The Series Instance UID every seeded image set holds") and the series rows at `:298-325`, `:334-341` all key on `SERIES_UID`; none drives an image set whose `dicomSeriesInstanceUID` is absent, and none names two different series UIDs in one document.

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE.
2. Get an image set into it that holds no DICOM content yet (as the crate's own suites do, seeding through the public `state` module with `ImageSet::new`, or as a `CopyImageSet` destination minted from a source with no delivered attributes) — its `dicomSeriesInstanceUID` is absent.
3. POST .../updateImageSetMetadata?latestVersion=1 with `updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes` = {"Study":{"Series":{"1.2.1":{"DICOM":{"Modality":"CT"}},"1.2.2":{"DICOM":{"Modality":"MR"}}}}} — it answers 200.
4. Wait IMAGE_SET_UPDATE_DELAY and call GetImageSetMetadata. Expected (per update_image_set_metadata.rs:29-33): each series' Modality is reported, the ones the modelled members cannot hold being kept at the path they were written at. Actual: the document contains no `Study.Series` level at all — "CT" was overwritten by "MR" in the single modelled member and both were removed from `extra_attributes`.
5. Call SearchImageSets on the data store: the summary's `DICOMTags.DICOMSeriesModality` reports "MR", so the search view and the metadata document disagree about the same image set.
6. Variant on a fully seeded image set: send `removableAttributes` = {"Study":{"Series":{<held uid>:{"DICOM":{"SeriesInstanceUID":<held uid>}}}}} with `force=true` (200), then GetImageSetMetadata — the document reports no series, while the still-stored Modality/BodyPartExamined/SeriesNumber keep showing up in SearchImageSets.
