Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# The served metadata document's DatastoreID/ImageSetID (and its Patient/Study levels) can be overwritten by caller-stored attributes

_Filed: 2026-10-28T07:21:59Z_
_Operation: GetImageSetMetadata | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImageSetMetadata` builds the aggregated document from state and then deep-merges the image set's `extra_attributes` over the WHOLE document, with no protection for the document's own structural keys. `merge_stored_attributes` replaces any key whose stored value is not an object (or whose document value is not an object), so a stored attribute named `DatastoreID` or `ImageSetID` takes the place of the computed identifier, and a stored scalar named `Patient` or `Study` replaces the whole level the handler just built — hiding the modelled DICOM attributes `SearchImageSets` still filters on.

Those attributes are trivially reachable: `UpdateImageSetMetadata` persists every attribute the model does not name at the exact document path it was written at, and its collector skips exactly ONE non-attribute document key (`SchemaVersion`) at the top level. `DatastoreID` and `ImageSetID` are non-attribute document keys by the very same reasoning the crate gives for `SchemaVersion` ("rather than naming an attribute of an image set"), yet they are stored and then reported back. The result is a 200 whose document claims an image set / data store identity that is not the one the request named, i.e. a caller-supplied value overriding computed state inside a read-only reporting operation.

## Evidence

The handler's own invariant — `src/ops/get_image_set_metadata.rs:276-294`:
```
/// The whole aggregated metadata document of `image_set`, as it stands in `datastore_id`.
///
/// Every value in it is read from state: the two identifiers name the records the document
/// was built from, and the attribute objects carry what has been stored for the image set
/// and nothing besides.
fn metadata_document(datastore_id: &str, image_set: &ImageSet) -> Value {
    let mut document = ::serde_json::json!({
        "SchemaVersion": METADATA_SCHEMA_VERSION,
        "DatastoreID": datastore_id,
        "ImageSetID": image_set.image_set_id,
...
    merge_stored_attributes(&mut document, &image_set.extra_attributes);
```
The merge that breaks it — `src/ops/get_image_set_metadata.rs:260-274`:
```
fn merge_stored_attributes(document: &mut Value, stored: &Map<String, Value>) {
    let Some(level) = document.as_object_mut() else {
        return;
    };
    for (name, value) in stored {
        match (level.get_mut(name), value) {
            (Some(held), Value::Object(nested)) if held.is_object() => {
                merge_stored_attributes(held, nested);
            }
            _ => {
                level.insert(name.clone(), value.clone());
            }
        }
    }
}
```
The write path that stores such a key — `src/ops/update_image_set_metadata.rs:544-545`:
```
            None if removing => remove_at(&mut attributes.extra_attributes, &at),
            None => write_at(&mut attributes.extra_attributes, &at, value),
```
and the one structural key it does exclude, with the reason that applies verbatim to the other two — `src/ops/update_image_set_metadata.rs:161-166` and `:403-406`:
```
/// The document key that declares which SHAPE of aggregated metadata document this is,
/// rather than naming an attribute of an image set — the one
/// `crate::ops::get_image_set_metadata` writes at the top of every document it serves. An
/// update document carries it to say how it is written, so there is nothing in it to
/// store.
const SCHEMA_VERSION_KEY: &str = "SchemaVersion";
```
```
        if path.is_empty() && name == SCHEMA_VERSION_KEY {
            continue;
        }
```
Test coverage states the rule but never exercises it: `tests/all/image_set_metadata_updates.rs:574-583`:
```
        // The identifiers the document names never move: an update writes attributes.
        for path in [vec!["DatastoreID"], vec!["ImageSetID"]] {
            assert_eq!(
                at(&after, &path),
                at(&before, &path),
```
yet no row of `update_cases()` sends a document containing `DatastoreID`/`ImageSetID` (the only structural-key row is `updatable: Some(json!({ "SchemaVersion": "1.1" }))`, `tests/all/image_set_metadata_updates.rs:437`), so the assertion passes trivially. `tests/all/image_set_metadata.rs:380-391` pins the intended answer: "expected the document to name its data store" / "expected the document to name its image set".

## Steps to Reproduce

1. Create a data store, let it settle to ACTIVE, and get an image set into it (seeded as the crate's own suites do, or as a CopyImageSet destination).
2. Call UpdateImageSetMetadata on it with `updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes` = {"SchemaVersion":"1.1","ImageSetID":"not-an-image-set","DatastoreID":"not-a-datastore"} and latestVersion=1; it answers 200 (no member is refused and no force is needed).
3. Wait IMAGE_SET_UPDATE_DELAY, then call GetImageSetMetadata for that image set.
4. Expected (per get_image_set_metadata.rs:278-280 and tests/all/image_set_metadata.rs:380-391): the document's `DatastoreID`/`ImageSetID` name the records it was built from. Actual: the document reports `"ImageSetID":"not-an-image-set"` and `"DatastoreID":"not-a-datastore"`.
5. Repeat with updatableAttributes = {"Patient":"x"}: the served document's whole `Patient` level is replaced by the string "x", so the modelled patient attributes the image set still holds (and SearchImageSets still filters on) are absent from the document.
