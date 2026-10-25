Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 558d123cf10a98531c236bb7b81525d860a40117

# GetImageFrame restates two state-module rules inline instead of calling the helpers the crate already owns

_Filed: 2026-10-25T20:52:19Z_
_Operation: GetImageFrame | Class: maintainability_

> _Note: this issue was identified against the code base at commit `558d123cf10a98531c236bb7b81525d860a40117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImageFrame` is the only handler in this crate that spells out two rules the state module already exposes as single-source helpers, so the copies can drift from the originals:

1. "this image set has been deleted" is written as a direct comparison of `latest_version.image_set_state` against `ImageSetState::Deleted`, duplicating the body of `ImageSet::is_deleted()`; the sibling `GetImageSetMetadata`, which applies the identical rule for the identical reason, calls the helper.
2. the re-entry that records the access walks `store.datastores.get_mut(<id>)` directly, bypassing `ComAmazonawsMedicalimagingAhigatewayserviceState::datastore_in_mut`, the accessor whose doc says every id-keyed read and write goes through it "so one rule decides reachability for all of them rather than each handler deciding for itself". `GetImageSetMetadata`'s identical re-entry uses the accessor and says explicitly why.

No response and no existing test changes if both lines are routed through the helpers: the earlier `datastore_in` lookup in the same handler has already established the region and that the record is not a tombstone, so the helper resolves the same record.

## Evidence

The helpers, in the state module:

`src/state.rs:1079-1084`
```
/// Whether this image set has been deleted — the terminal state a delete lands on,
...
pub fn is_deleted(&self) -> bool {
    self.latest_version.image_set_state == ImageSetState::Deleted
}
```
`src/state.rs:1385-1410`
```
/// ... Every
/// id-keyed read and write goes through here (or through [`Self::datastore_in_mut`]),
/// so one rule decides reachability for all of them rather than each handler
/// deciding for itself.
...
pub fn datastore_in_mut(&mut self, region: &str, datastore_id: &str) -> Option<&mut Datastore> {
    self.datastores
        .get_mut(datastore_id)
        .filter(|datastore| datastore.region == region && !datastore.is_deleted())
}
```

The copies, in `GetImageFrame`:

`src/ops/get_image_frame.rs:187-196`
```
// A DELETED image set's pixel data went with it, so what is absent is the image
// set itself, not one frame of it.
if image_set.latest_version.image_set_state == crate::state::ImageSetState::Deleted {
```
`src/ops/get_image_frame.rs:221-227`
```
if let Some(image_set) = store
    .datastores
    .get_mut(&input.datastore_id)
    .and_then(|datastore| datastore.image_sets.get_mut(&input.image_set_id))
{
    image_set.record_access(now);
}
```

The sibling that uses both helpers, for the same two decisions:

`src/ops/get_image_set_metadata.rs:367-377`
```
// A DELETED image set holds no content, so what is absent is the image set
// itself rather than one attribute of it — the same answer `GetImageFrame` gives
// for the pixel data that went with the deletion.
if image_set.is_deleted() {
```
`src/ops/get_image_set_metadata.rs:411-419`
```
// ... and reached through the REGION this
// request was addressed to, so the record written is the one that was read.
if let Some(image_set) = store
    .datastore_in_mut(&region, &input.datastore_id)
    .and_then(|datastore| datastore.image_sets.get_mut(&input.image_set_id))
```

Existing coverage that must stay green unchanged: `tests/all/image_frames.rs` ("RULE: `GetImageFrame` serves the pixel data the image set holds under the image frame …", including the 404 rows at line 343 and the deleted-image-set rows around line 638) and `tests/all/image_set_access_time.rs:228` ("serving an image set's pixel data accesses the image set").

## Steps to Reproduce

1. Read src/ops/get_image_frame.rs:189 and compare with src/state.rs:1082-1084 — the same comparison, written twice.
2. Read src/ops/get_image_frame.rs:221-224 and compare with src/state.rs:1406-1410 and src/ops/get_image_set_metadata.rs:414-416 — the same mutable lookup, once through the region-scoping accessor and once past it.
3. Route both lines through `ImageSet::is_deleted()` and `datastore_in_mut(&region, ..)` and run the crate's tests: every assertion in tests/all/image_frames.rs and tests/all/image_set_access_time.rs still holds.
