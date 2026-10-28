Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# A channel's pinned engine version is reported with no expirationDate on the claim that the emulator does not know it, while ListVersions reports that very expiry for the same version string

_Filed: 2026-10-28T19:03:06Z_
_Operation: DescribeChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`described_engine_version` builds the `ChannelEngineVersionResponse` that every channel-shaped reply carries (`channel_description` and `channel_summary`). It hard-codes `expiration_date: None` and justifies it with the comment "an AWS-side release fact, not something this emulator knows".

That justification is false inside this crate. The engine-version catalogue is this emulator's OWN: `engine_versions.rs` says so in as many words, it stores an `expires_at` instant per version, and `ListVersions` reports exactly that instant as the `expirationDate` of each published version. The catalogue is reachable from `channel_description` — `channel_engine_versions` is a public field of the same state struct the method is defined on, keyed by the very build identifier the channel stores — so the expiry of a channel pinned to a published version is a fact the store already holds and already serves through another operation.

Result: a caller who takes a `version` out of `ListVersions` (which reports it with an `expirationDate`) and pins it on a channel gets that same version back from every channel read with `expirationDate` absent. The two readers of one stored fact disagree, and the comment asserting the fact is unknown is contradicted by the sibling slice that stores and serves it.

## Evidence

The projection and its justification — services/service-com-amazonaws-medialive-medialive/src/state.rs:5924-5932:
```
/// `ChannelEngineVersionRequest` (just a `Version`) -> the response shape.
/// `ExpirationDate` is "The UTC time when the version expires" — an AWS-side
/// release fact, not something this emulator knows, so it stays absent.
fn described_engine_version(version: &str) -> crate::ops::types::ChannelEngineVersionResponse {
    crate::ops::types::ChannelEngineVersionResponse {
        expiration_date: None,
        version: Some(version.to_string()),
    }
}
```
reached from services/service-com-amazonaws-medialive-medialive/src/state.rs:3247-3250 (`channel_description`, what DescribeChannel answers from) and state.rs:3283-3286 (`channel_summary`).

The same crate stores and serves that instant — services/service-com-amazonaws-medialive-medialive/src/state/engine_versions.rs:25-32:
```
//! The three facts AWS publishes nowhere machine-readable — how many versions an
//! account is offered, how far apart they are released and how long one lasts —
//! are this EMULATOR's own ([`CHANNEL_ENGINE_VERSION_COUNT`],
//! [`CHANNEL_ENGINE_VERSION_INTERVAL`], [`CHANNEL_ENGINE_VERSION_LIFETIME`])
```
services/service-com-amazonaws-medialive-medialive/src/state/engine_versions.rs:95-96:
```
/// When it expires, as `ExpirationDate` reports it.
pub expires_at: SystemTime,
```
services/service-com-amazonaws-medialive-medialive/src/state/engine_versions.rs:134-141:
```
/// The modelled `ChannelEngineVersionResponse` for this entry: its stored build
/// identifier and its stored expiry.
pub fn view(&self) -> ChannelEngineVersionResponse {
    ChannelEngineVersionResponse {
        expiration_date: Some(::aws_smithy_types::DateTime::from(self.expires_at)),
        version: Some(self.version.clone()),
    }
}
```
and it is already served by services/service-com-amazonaws-medialive-medialive/src/ops/list_versions.rs:163-165:
```
Ok(ListVersionsOutput {
    versions: Some(state.channel_engine_versions.available_at(now)),
})
```

The catalogue is reachable from the channel projection: services/service-com-amazonaws-medialive-medialive/src/state.rs:2465 declares `pub channel_engine_versions: ChannelEngineVersionCatalog` on the same state struct `channel_description` is an inherent method of, and services/service-com-amazonaws-medialive-medialive/src/state/engine_versions.rs:152-155 declares `pub versions: BTreeMap<String, ChannelEngineVersionRecord>` keyed by the build identifier.

## Steps to Reproduce

1. ListVersions — note a published entry, e.g. `{"version": "1.3.0", "expirationDate": <instant>}`.
2. CreateChannel with `channelEngineVersion: {"version": "1.3.0"}`.
3. DescribeChannel for the returned channel id.
4. Observed: `channelEngineVersion` is `{"version": "1.3.0"}` with no `expirationDate`, although step 1 reported an `expirationDate` for that exact version out of the same store.
5. Expected: the channel reports the expiry the catalogue holds for the version it is pinned to, so the two readers of one stored instant agree (a version string the catalogue does not hold legitimately has no expiry to report).
