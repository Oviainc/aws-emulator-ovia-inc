Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ebf0e2f135a02ae75ffd55898f9ee21fc25614a

# A copy into an existing destinationImageSet discards every DICOM attribute the destination already held

_Filed: 2026-10-28T20:09:22Z_
_Operation: CopyImageSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ebf0e2f135a02ae75ffd55898f9ee21fc25614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `copyImageSetInformation.destinationImageSet` names an image set the data store already holds, the copy's pending transition carries the SOURCE's attributes as `delivered_attributes`, and the clock applies them with `ImageSet::set_attributes`, which REPLACES both halves of the destination's attribute state (`dicom_tags` and `extra_attributes`) rather than merging them. Everything the destination held and the source does not — patient/study/series attributes, every attribute outside the sixteen the model names, and every per-SOP-instance attribute a prior `UpdateImageSetMetadata` wrote into it — is silently dropped.

Two things in this crate contradict that erasure:

1. The same transition handles the destination's PIXEL DATA by MERGING (`image_frames.extend(delivered)`), so after a copy the destination serves the union of its own frames and the copied ones, but describes only the source's content. Its own frames are left in the served metadata document with nothing but the `SOPInstanceUID` the frame record carries, because the per-instance attributes that described them lived in `extra_attributes` and were thrown away. Frames merge, attributes replace — one transition, two opposite rules for the same destination.

2. The handler's own doc justifies admitting an unforced copy whose metadata only PARTLY overlaps on the premise that "the copy supplies" the element the destination has not got. The inconsistency guard refuses only elements BOTH sides hold with differing values (`inconsistent_metadata` compares the two `DICOMTags` pairwise and `disagrees` returns false when either side is absent), and it never looks at `extra_attributes` at all. So exactly the request the guard waves through on the premise that nothing is lost is the request that loses the destination's own attributes.

This is distinct from the already-fixed CopyImageSet wrong-behavior issues (promoteToPrimary applied as a flag; DELETED image sets counted toward the capacity bound). A minted destination is unaffected — it holds nothing — so the defect is specific to the `destinationImageSet` variant, which is the only path that writes into an image set a caller already has content in.

## Evidence

The copy hands the source's attributes to the destination's transition — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/copy_image_set.rs:730-734`:
```
            delivered_image_frames: Some(copied_image_frames),
            // The destination describes the source's patient, study and series from the
            // same instant: the attributes a search filters on are delivered with the
            // content they describe.
            delivered_attributes: Some(copied_attributes),
```

The clock then MERGES the frames but REPLACES the attributes — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:1328-1336`:
```
        if let Some(delivered) = pending.delivered_image_frames {
            self.image_frames.extend(delivered);
        }
        ...
        if let Some(delivered) = pending.delivered_attributes {
            self.set_attributes(delivered);
        }
```
and `set_attributes` is a wholesale replacement of both halves — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:1274-1277`:
```
    pub fn set_attributes(&mut self, attributes: ImageSetAttributes) {
        self.dicom_tags = attributes.dicom_tags;
        self.extra_attributes = attributes.extra_attributes;
    }
```

The handler doc that justifies admitting such a copy — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/copy_image_set.rs:39-42`:
```
//!   (409, [`crate::state::IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE`]). An element only
//!   one of the two holds is no disagreement — the copy supplies it — so an image set that
//!   holds no DICOM content yet takes any copy, and a MINTED destination holds nothing to
//!   disagree with.
```
and the guard it describes, which compares only the modelled tags and treats a one-sided element as fine — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/copy_image_set.rs:648` and `:284-289`:
```
                if !forced && inconsistent_metadata(&copied_attributes.dicom_tags, &held.dicom_tags)
```
```
fn disagrees<T: PartialEq>(copied: Option<&T>, held: Option<&T>) -> bool {
    match (copied, held) {
        (Some(copied), Some(held)) => copied != held,
        _ => false,
    }
}
```

Test coverage: no test pins a copy into a destination that holds attributes. `tests/all/image_set_copy.rs` seeds destinations through `seed_image_set`, which sets no `dicom_tags`/`extra_attributes` at all, and `tests/all/image_set_copy_subsets.rs:206-213` only ever copies into a MINTED destination (`information("1", case.copiable_attributes.as_deref(), None)`), so the destination-versioning path is never exercised with content on the destination. A fix therefore changes no existing assertion.

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE.
2. Seed/obtain two ACTIVE image sets A (the source) and B (the destination) in it, as `tests/all/image_set_copy.rs::seed_image_set` does.
3. POST UpdateImageSetMetadata on B with `updatableAttributes` = {"Patient":{"DICOM":{"PatientName":"B-ONLY"}},"Study":{"Series":{"<B-series>":{"Instances":{"<B-instance>":{"DICOM":{"00091001":"private-B"}}}}}}} and let the update settle.
4. POST UpdateImageSetMetadata on A with `updatableAttributes` = {"Patient":{"DICOM":{"PatientID":"A-ONLY"}}} and let it settle.
5. POST CopyImageSet with sourceImageSetId=A and `copyImageSetInformation.destinationImageSet` naming B at its current latestVersionId (no `force`). It is admitted: no element is held by both with differing values.
6. Wait for IMAGE_SET_COPY_DELAY, then POST GetImageSetMetadata on B.
7. Observed: B's document reports only A's attributes — `PatientName` "B-ONLY" and the private instance attribute "private-B" are gone, while GetImageFrame still serves B's original frames. Expected: the copy supplies what B had not got rather than erasing what B held.
