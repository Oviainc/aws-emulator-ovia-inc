Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# CopyImageSet counts DELETED image sets toward the per-data-store image-set bound, so deletions permanently consume capacity

_Filed: 2026-10-28T05:27:40Z_
_Operation: CopyImageSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The mint path of `CopyImageSet` counts `datastore.image_sets.len()` — every record the map holds, tombstones included — against `MAX_IMAGE_SETS_PER_DATASTORE`, and refuses with `ServiceQuotaExceededException` (402) when the count is reached. Image-set records are never removed from the map (no `image_sets.remove`/`retain` anywhere in the crate), so an image set that has been deleted keeps occupying its slot forever: after creating and deleting image sets `MAX_IMAGE_SETS_PER_DATASTORE` times, a data store that holds no live image set at all can never be copied into again. Every other place in this crate that counts or walks image sets/data stores for work explicitly excludes tombstones, and the state module states the rule for the equivalent data-store bound: a tombstone "neither takes a quota slot nor can be worked on". `CopyImageSet` is the one counter that ignores it.

## Evidence

The count that includes tombstones — `src/ops/copy_image_set.rs:567`:
```
        let held_image_sets = datastore.image_sets.len();
```
fed to the bound at `src/ops/copy_image_set.rs:626-639`:
```
                // The same per-data-store memory bound the import jobs carry
                // (`crate::state::MAX_IMAGE_SETS_PER_DATASTORE`), refused with this
                // operation's modelled quota error.
                if held_image_sets >= crate::state::MAX_IMAGE_SETS_PER_DATASTORE {
                    return Err(CopyImageSetErrorModeled::ServiceQuotaExceededException(
```
The rule the crate states for the same kind of bound — `src/state.rs:1555-1563`:
```
    /// Every data store `region` holds that work can still reach, in no particular
    /// order — what the per-region quota counts.
    ///
    /// A tombstone occupies no place: a data store whose deletion has landed has given
    /// up everything it held, so it neither takes a quota slot nor can be worked on.
    pub fn datastores_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Datastore> {
        self.reported_datastores_in(region)
            .filter(|datastore| !datastore.is_deleted())
```
The sibling that counts image sets and filters tombstones out, with its reason — `src/ops/delete_datastore.rs:179-188`:
```
        // Image sets the data store still HOLDS block its deletion; ones already deleted
        // do not, because a deleted image set is a tombstone — it holds no pixel data and
        // takes part in no further work ([`crate::state::ImageSet::is_deleted`]). Without
        // that distinction, deleting every image set in a store would never unblock the
        // store's own delete.
        let held_image_sets = datastore
            .image_sets
            .values()
            .filter(|image_set| !image_set.is_deleted())
            .count();
```
Nothing ever reaps an image-set record: a repo-wide grep for `image_sets.remove` / `image_sets.retain` in `src/` returns no match (only `image_sets.len()` at copy_image_set.rs:567). No test covers this bound (`MAX_IMAGE_SETS_PER_DATASTORE` appears in no file under `tests/`).

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE; seed one image set as the crate's tests do.
2. Repeat `MAX_IMAGE_SETS_PER_DATASTORE` times: `CopyImageSet` (no `destinationImageSet`) to mint a destination, wait for IMAGE_SET_COPY_DELAY, then `DeleteImageSet` the destination and wait for IMAGE_SET_DELETION_DELAY.
3. Verify via `SearchImageSets` that the data store reports no live image set beyond the seeded one.
4. Issue another `CopyImageSet` with no `destinationImageSet`: it answers 402 `ServiceQuotaExceededException` ("this emulator holds at most 128 image sets per data store") even though every counted record is a tombstone that, per src/state.rs:1558-1559, "neither takes a quota slot nor can be worked on".
