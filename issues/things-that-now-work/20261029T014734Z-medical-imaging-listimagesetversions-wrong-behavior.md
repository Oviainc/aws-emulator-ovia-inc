Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# Earlier versions keep reporting ACTIVE while a copy or a metadata update holds the image set LOCKED

_Filed: 2026-10-29T01:47:34Z_
_Operation: ListImageSetVersions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states — in the state module, in the delete handler and in a test — that `ImageSetState` is a property of the IMAGE SET and not of one snapshot of it, and it owns a helper (`ImageSet::set_state_on_every_version`) that applies a state change to every version for exactly that reason. `DeleteImageSet` calls that helper when it locks an image set; `CopyImageSet` and `UpdateImageSetMetadata` do not. They lock by putting `ImageSetState::Locked` on the NEW version only (`push_version`), and `push_version` moves the previous latest into `earlier_versions` carrying the state it already had (`ACTIVE`).

So while a copy or a metadata update is in flight — the window the lock exists for — `ListImageSetVersions` enumerates version 1..N-1 as `ACTIVE` and only version N as `LOCKED`, and `GetImageSet?version=<earlier>` answers `ACTIVE` too. That is the very thing the delete path's comment forbids ("no version is left reporting `ACTIVE` while the image set is on its way out"), and it contradicts the service's own behaviour at that instant: every write path refuses the image set with `ConflictException` / `IMAGE_SET_NOT_ACTIVE_MESSAGE` computed from `latest_version.image_set_state`, so a caller is told `ACTIVE` by one reader and "not in ACTIVE state" by the next write. The source image set of a copy has the same gap: only `source.latest_version.image_set_state` is set to `Locked`.

The fix is for the two in-flight lock paths to reach every version the way the delete path already does (and, symmetrically, for the clock's release to put the whole image set back). Nothing is wrong once the transition has landed: after release every version reads `ACTIVE` again, so the defect is confined to the in-flight window.

## Evidence

The rule and the helper this crate already owns:

- `src/state.rs:1342-1352`: "/// Put EVERY version of this image set into `image_set_state`. /// /// Used where a change concerns the whole image set rather than one snapshot of it — /// its retirement — so that no version is left reporting a state the image set no /// longer has." → `pub fn set_state_on_every_version(&mut self, image_set_state: ImageSetState)`
- `src/state.rs:1393-1398` (in `ImageSet::advance_to`): "// Retiring the image set reaches EVERY version of it. `ImageSetState` is // whether the image set can be read and written, which is a property of the // image set rather than of one snapshot of it, so no version of a retired // image set is left reporting a state it no longer has"
- `src/ops/delete_image_set.rs:212-216`: "// The lock reaches every version: a delete is about the whole image set, not // about one snapshot of it, so no version is left reporting `ACTIVE` while // the image set is on its way out." → `image_set.set_state_on_every_version(ImageSetState::Locked);`

The two sibling write paths that skip that helper:

- `src/ops/copy_image_set.rs:771-776`: `image_set.push_version(ImageSetState::Locked, ImageSetWorkflowStatus::Copying, forced, now,);` (destination) and `src/ops/copy_image_set.rs:797-798`: `source.latest_version.image_set_state = ImageSetState::Locked; source.latest_version.workflow_status = ImageSetWorkflowStatus::CopyingWithReadOnlyAccess;` (source)
- `src/ops/update_image_set_metadata.rs:1110`: `image_set.push_version(ImageSetState::Locked, workflow_status, forced, now);`
- `src/state.rs:1296-1309` shows the superseded version keeps its own state: `let mut superseded = ::std::mem::replace(&mut self.latest_version, ImageSetVersion { version_id, image_set_state, ... }); ... self.earlier_versions.push(superseded);`

The readers that surface the stale state:

- `src/ops/list_image_set_versions.rs:225`: `image_set_state: version.image_set_state.as_wire().to_string(),`
- `src/ops/get_image_set.rs:205`: `image_set_state: version.image_set_state.as_wire().to_string(),`

The contradiction at the same instant (refusals read the LATEST version):

- `src/ops/copy_image_set.rs:569-574`: `if source.latest_version.image_set_state != ImageSetState::Active { return Err(... ConflictException { message: crate::state::IMAGE_SET_NOT_ACTIVE_MESSAGE ...`
- `src/ops/update_image_set_metadata.rs:757-760`: `fn is_updatable(image_set: &ImageSet) -> bool { image_set.latest_version.image_set_state == ImageSetState::Active && image_set.pending.is_none() }`

Test coverage: the delete side of the rule IS pinned — `tests/all/image_set_deletion.rs:296-300` asserts, for EVERY version, `"LOCKED", "case {name:?} version {version}: expected LOCKED in flight, got {read:?}"`. The copy side is only asserted AFTER the clock has landed the copy — `tests/all/image_set_version_listing.rs:531-537`: `entry["imageSetState"].as_str(), Some("ACTIVE"), "case {case:?}: expected ACTIVE once the copy the clock owed has landed"` (preceded by `tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY)` at line 506), so no test pins the in-flight window for a copy or an update.

## Steps to Reproduce

1. CreateDatastore and wait for it to settle into ACTIVE.
2. Obtain an image set with at least two versions in that data store (e.g. CopyImageSet with no destination to mint one, let it settle, then CopyImageSet again naming that image set as `destinationImageSet` and let it settle).
3. Start another CopyImageSet into that image set (or UpdateImageSetMetadata on it) and, within `IMAGE_SET_COPY_DELAY` / `IMAGE_SET_UPDATE_DELAY` (400 ms), call ListImageSetVersions for it.
4. Observe the latest version reports `imageSetState: LOCKED` with `COPYING`/`UPDATING`, while every earlier version still reports `imageSetState: ACTIVE`; `GetImageSet?version=<earlier>` reports `ACTIVE` as well.
5. In the same window, call DeleteImageSet or CopyImageSet on that image set: it is refused 409 with "the image set is not in ACTIVE state" — computed from the latest version — so the earlier versions' `ACTIVE` contradicts the service's own answer.
6. Compare with a DeleteImageSet in flight, where tests/all/image_set_deletion.rs pins EVERY version at `LOCKED`.
