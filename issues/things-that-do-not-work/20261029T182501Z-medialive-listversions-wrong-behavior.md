Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: fad1fdecc61c47880f0bf921597b8afa67a7a57d

# The engine-version catalogue is seeded per REGION partition at first touch, so one account's version "1.0.0" reports a different ExpirationDate through each regional endpoint

_Filed: 2026-10-29T18:25:01Z_
_Operation: ListVersions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fad1fdecc61c47880f0bf921597b8afa67a7a57d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`channel_engine_versions` is a member of the per-REGION partition struct (`ComAmazonawsMedialiveMedialiveState`), not of the account-wide store (`ComAmazonawsMedialiveMedialiveStore`). A partition is created lazily, the first time a request reaches this service in that region (`regions.rs::region` -> `RegionState::new` -> `ComAmazonawsMedialiveMedialiveState::with_mints` -> `..Self::default()`), and the catalogue's `Default` seeds it as of `SystemTime::now()`. So the catalogue is seeded once per REGION, at whatever instant that region was first touched, rather than once for the account.

Two consequences, both observable over the wire:

* The build identifiers are region-independent (`channel_engine_version_label(release)` -> "1.0.0" .. "1.3.0"), but `released_at`/`expires_at` are not: `ListVersions` signed for us-east-1 and `ListVersions` signed for us-west-2 report the SAME `version` strings with DIFFERENT `expirationDate` instants, offset by the time between the two regions' first requests. The same divergence reaches `DescribeChannel`/`CreateChannel`, which report a pinned version through `ChannelEngineVersionCatalog::described` out of the partition they run in.
* Which versions are *available* becomes a function of when a region was first touched rather than of the account: an entry reported as expired in a long-lived region is still reported as available through a region whose partition was created later.

This contradicts the slice's own two statements (the catalogue is the ACCOUNT's and is seeded when the ACCOUNT's store comes up; a stored expiry means "two reads of one version report the same instant"), and it is the opposite of how this crate places the other piece of non-resource, account-level state: the account configuration deliberately "sits beside the region partitions rather than inside one because it is not a resource a caller creates". The engine-version slice states exactly the same premise about itself and is nevertheless held inside a partition.

## Evidence

The catalogue is a member of the REGION partition — services/service-com-amazonaws-medialive-medialive/src/state.rs:2523-2530:
```
    /// The account's encoder engine version catalogue, as `ListVersions` reports
    /// the available ones out of it.
    ///
    /// There is no mint counter and no token map beside it: an engine version is
    /// not a resource a caller creates, it is one the service publishes, so the
    /// catalogue is SEEDED when this store comes up (the [`engine_versions`]
    /// slice's own `Default`) and no request path writes it.
    pub channel_engine_versions: ChannelEngineVersionCatalog,
```
and that struct IS one region's partition — src/state/regions.rs:122-135:
```
pub struct RegionState(Arc<Mutex<ComAmazonawsMedialiveMedialiveState>>);
...
    fn new(mints: Arc<IdMints>) -> Self {
        Self(Arc::new(Mutex::new(
            ComAmazonawsMedialiveMedialiveState::with_mints(mints),
        )))
    }
```
with the partition created on first touch — src/state/regions.rs:89-103 (`fn region(&mut self, region: &str) -> Option<RegionState>` … `.or_insert_with(|| RegionState::new(mints))`).

The seed is the WALL CLOCK of that moment — src/state/engine_versions.rs:158-163:
```
impl Default for ChannelEngineVersionCatalog {
    /// The catalogue of an account whose store has just come up, seeded as of now.
    fn default() -> Self {
        Self::seeded_at(SystemTime::now())
    }
}
```
and src/state/engine_versions.rs:173-182 dates every entry off that instant (`now.checked_sub(age)?`), with the expiry derived from it (src/state/engine_versions.rs:106-113, `expires_at: released_at.checked_add(CHANNEL_ENGINE_VERSION_LIFETIME)`).

The statements the placement breaks — src/state/engine_versions.rs:12-17:
```
//! An engine version is NOT a resource a caller creates: `ListVersions`
//! "retrieves an array of all the encoder engine versions that are available in
//! this AWS account", and no operation of this service creates, deletes or
//! changes one. So the catalogue is SEEDED when the account's store comes up and
//! no request path writes it, which is what makes the versions a caller reads
//! back the ones that were persisted rather than ones a handler assembled.
```
and src/state/engine_versions.rs:84-87:
```
/// Both instants are stored, so the `ExpirationDate` a caller reads is the one the
/// catalogue was built with rather than a value recomputed per request — two reads
/// of one version report the same instant.
```

The crate's own precedent for this exact kind of state — src/state/regions.rs:59-73:
```
    /// The account's MediaLive configuration in each region an
    /// `UpdateAccountConfiguration` has written one in, keyed by the region the
    /// writing request was signed for.
    ...
    /// It sits beside the region partitions rather than inside one because it is
    /// not a resource a caller creates — it is the account's own settings, so a
    /// region nothing has configured yet reads as the unset configuration rather
    /// than as missing, and its bound is on the configurations the whole store
    /// keeps.
```

The read path that exposes it — src/ops/list_versions.rs:156-165:
```
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(unavailable)?;
        let now = ::std::time::SystemTime::now();
        let state = state.lock();
        Ok(ListVersionsOutput {
            versions: Some(state.channel_engine_versions.available_at(now)),
        })
```
(`service_state` is documented at src/state/regions.rs:164-171 as "the resources of the region `parts` was signed for … This is the ONLY store an operation on a resource is given".)

Test coverage: tests/all/engine_version_catalog.rs (installs a catalogue through `state::service_state_in(engine, DEFAULT_REGION).lock().channel_engine_versions`, lines 66-69, and asserts the reported entries/order/expiries) and tests/all/channel_engine_version_expiry.rs (a channel reports the entry `ListVersions` published). Neither asserts anything about a SECOND region's catalogue — tests/all/region_isolation.rs has no engine-version row — so no test pins the per-region seeding; a fix that holds one catalogue for the account leaves every assertion intact (only the `install`/`stored_versions` helpers' reach changes).

## Steps to Reproduce

1. Start the emulator and call GET /prod/versions signed for us-east-1 — note the `expirationDate` reported for version "1.3.0" (call it E1). This creates the us-east-1 partition and seeds its catalogue at that instant (src/state/regions.rs:89-103, src/state/engine_versions.rs:158-163).
2. Wait a noticeable interval (or create any resource in a second region first), then call GET /prod/versions signed for us-west-2 — the same version "1.3.0" is reported with a DIFFERENT `expirationDate` E2, offset by the interval, because that partition's catalogue was seeded when it came up.
3. Compare with src/state/engine_versions.rs:84-87 ("two reads of one version report the same instant") and src/state/engine_versions.rs:12-17 ("the catalogue is SEEDED when the account's store comes up"): one account's published catalogue now depends on when each regional endpoint was first reached.
4. Pin a channel on "1.3.0" in each region and GET /prod/channels/{id}: the `channelEngineVersion.expirationDate` each region reports for the same build identifier differs too (ChannelEngineVersionCatalog::described, src/state/engine_versions.rs:212-220).
5. Contrast the placement with the account configuration, which the crate deliberately holds on the store beside the partitions for the same stated reason (src/state/regions.rs:59-73, src/state/account_configuration.rs:11-25).
