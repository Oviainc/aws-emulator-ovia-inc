Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# GetImageSetMetadata validates the `version` query parameter and then drops it: every version serves the LATEST attributes

_Filed: 2026-10-28T16:07:37Z_
_Operation: GetImageSetMetadata | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImageSetMetadata` accepts `versionId` (`@httpQuery("version")`), uses it only to decide 404-or-not, and then builds the document from the image set's CURRENT attributes. The requested version never selects the content served, so `?version=1` on an image set now at version 3 returns version 3's document.

This contradicts the crate's own state model, which persists a per-version snapshot of the DICOM attributes precisely so that a version can be served what it held, and already exposes the helper that reads it (`ImageSet::attributes_of_version`). `UpdateImageSetMetadata`'s `revertToVersionId` path calls that helper; `GetImageSetMetadata` — the one operation that takes a version identifier for the metadata document — does not. The two cannot both be right: if "the content aggregated belongs to the image SET" so that "every version it holds reports the same document", then there is nothing for a revert to put back.

Concretely: seed an image set, write attribute A at version 1, then UpdateImageSetMetadata (v1 -> v2) writing attribute B. `GetImageSetMetadata?version=1` reports B, while `UpdateImageSetMetadata` with `revertToVersionId="1"` proves the service still holds the v1 snapshot with A and without B. The reader therefore answers 200 with a document the version it names never held.

## Evidence

The handler reads `versionId` only to probe existence, and says so — `src/ops/get_image_set_metadata.rs:413-425`:
```
        // `version` names ONE version of the image set; a version this image set does
        // not hold is absent, not invalid. The content aggregated below belongs to the
        // image SET, so every version it holds reports the same document.
        if let Some(version_id) = &input.version_id {
            if image_set.version(version_id).is_none() {
                return Err(GetImageSetMetadataErrorModeled::ResourceNotFoundException(
```
and then the version is never used again — `src/ops/get_image_set_metadata.rs:427`:
```
        let document = metadata_document(&input.datastore_id, image_set);
```
which reads the image set's CURRENT attributes — `src/ops/get_image_set_metadata.rs:281-292`:
```
fn metadata_document(datastore_id: &str, image_set: &ImageSet) -> Value {
    let mut document = ::serde_json::json!({
...
        "Patient": { "DICOM": patient_attributes(&image_set.dicom_tags) },
...
    merge_stored_attributes(&mut document, &image_set.extra_attributes);
```
The other side of the contradiction, in this same crate — `src/state.rs:1004-1010`:
```
    /// The DICOM attributes this version held, recorded at the instant a LATER version
    /// superseded it — what reverting the image set to this version puts back.
    ///
    /// `None` on the LATEST version, whose attributes are the ones the image set holds
    /// now ([`ImageSet::dicom_tags`], [`ImageSet::extra_attributes`]), so a version and
    /// the image set can never disagree about the content the image set is serving.
    pub superseded_attributes: Option<ImageSetAttributes>,
```
and the helper that already reads it — `src/state.rs:1259-1262`:
```
    /// The DICOM attributes the version `wire_version_id` (`^\d+$`) held, or `None` when
    /// this image set holds no such version. The latest version's are the ones the image
    /// set holds now; an earlier version's are the snapshot taken when it was superseded.
    pub fn attributes_of_version(&self, wire_version_id: &str) -> Option<ImageSetAttributes> {
```
The sibling path that DOES call it — `src/ops/update_image_set_metadata.rs:839-841`:
```
            UpdatePlan::Revert(version_id) => {
                let attributes = image_set
                    .attributes_of_version(version_id)
```
with its handler doc — `src/ops/update_image_set_metadata.rs:26-28`:
```
//!   * `revertToVersionId` names "the previous image set version ID to revert the current
//!     image set back to": the attributes that version held
//!     ([`crate::state::ImageSet::attributes_of_version`]) become the image set's again.
```
Test coverage asserts the current (restated) behaviour rather than justifying it, and seeds versions whose snapshots DO differ (`push_version` snapshots before `dicom_tags` is assigned) — `tests/all/image_set_metadata.rs:542-553`:
```
        // Every version the image set holds aggregates the same content, and one past
        // its latest is absent.
        for version in 1..=case.versions {
            let for_version = get_image_set_metadata(
...
            assert_document(&for_version, &case, &datastore_id, &image_set_id, &frame_ids);
```
(`tests/all/image_set_metadata.rs:214-222` is the seed that pushes the versions before setting the attributes.) `tests/all/image_set_metadata_updates.rs:1033-1126` (`a_revert_puts_back_the_attributes_the_version_it_names_held`) pins the opposite premise: each version held different attributes.

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE; get an image set into it (seeded through the crate's public `state` module, as every image-set suite in this crate does).
2. Call UpdateImageSetMetadata on the image set with `latestVersion=1` and `updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes` = {"Patient":{"DICOM":{"PatientName":"after"}}}; it answers 200 at version 2.
3. Wait IMAGE_SET_UPDATE_DELAY so the update settles, then call GetImageSetMetadata with `?version=1`.
4. Expected: the document version 1 held (PatientName as it was before step 2, per `ImageSetVersion::superseded_attributes` / `ImageSet::attributes_of_version`). Actual: the document reports "PatientName":"after" — version 2's content.
5. Confirm the snapshot really exists and differs: call UpdateImageSetMetadata with `latestVersion=2` and `{"revertToVersionId":"1"}`; it answers 200 and restores the pre-update attributes, which only the v1 snapshot could supply.
