Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# CreateChannel stores channelEngineVersion.version = "DEFAULT" as a build identifier instead of resetting the pin, the one reading its sibling UpdateChannel implements

_Filed: 2026-10-28T19:03:06Z_
_Operation: CreateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateChannel`'s `ChannelEngineVersion` targets `ChannelEngineVersionRequest`, whose `Version` member is documented "The build identifier of the engine version to use for this channel. Specify 'DEFAULT' to reset to the default version." `UpdateChannel` implements that sentence: it compares the requested value against a named `DEFAULT_ENGINE_VERSION` constant and clears the stored pin instead of writing the literal. `CreateChannel` writes the request's string straight onto the record with no such branch, so a create carrying `{"channelEngineVersion":{"version":"DEFAULT"}}` stores `"DEFAULT"` as the channel's pinned build identifier.

Consequences: every later read of the channel (`DescribeChannel`, `ListChannels`, `StartChannel`, …) reports `channelEngineVersion.version = "DEFAULT"`, a value that is not one of the build identifiers `ListVersions` publishes, instead of reporting the channel as un-pinned the way a channel created with no `ChannelEngineVersion` at all is reported. The two sibling writers of the same record field therefore disagree about what the same request value means: `UpdateChannel` with `DEFAULT` un-pins a channel, `CreateChannel` with `DEFAULT` pins it to a nonexistent version.

## Evidence

Smithy model (`smithy_query(query="doc", shape="ChannelEngineVersionRequest", member="Version")`):
  "The build identifier of the engine version to use for this channel. Specify 'DEFAULT' to reset to the default version."

The sibling that implements it — services/service-com-amazonaws-medialive-medialive/src/ops/update_channel.rs:356-366:
```
if let Some(version) = input.channel_engine_version.and_then(|version| version.version) {
    // "Specify 'DEFAULT' to reset to the default version": the reset
    // un-pins the channel, which is the state a channel created without
    // an engine version is in, so the stored pin is cleared rather than
    // being set to the literal the caller typed.
    channel.channel_engine_version = if version == DEFAULT_ENGINE_VERSION {
        None
    } else {
        Some(version)
    };
}
```
and services/service-com-amazonaws-medialive-medialive/src/ops/update_channel.rs:388-390:
```
/// The `ChannelEngineVersionRequest$Version` value that is not a build
/// identifier but a reset: "Specify 'DEFAULT' to reset to the default version".
const DEFAULT_ENGINE_VERSION: &str = "DEFAULT";
```

The create that skips it — services/service-com-amazonaws-medialive-medialive/src/ops/create_channel.rs:319-320:
```
channel.channel_engine_version =
    input.channel_engine_version.and_then(|version| version.version);
```
The `DEFAULT_ENGINE_VERSION` constant is private to `ops::update_channel` and is not referenced anywhere else in the crate (`grep -n 'DEFAULT_ENGINE_VERSION'` matches only update_channel.rs:361 and update_channel.rs:390).

The stored value is reported verbatim — services/service-com-amazonaws-medialive-medialive/src/state.rs:3247-3250:
```
channel_engine_version: channel
    .channel_engine_version
    .as_deref()
    .map(described_engine_version),
```

## Steps to Reproduce

1. CreateChannel with a valid configuration and body member `channelEngineVersion: {"version": "DEFAULT"}`.
2. DescribeChannel for the returned channel id.
3. Observed: `channelEngineVersion.version` is the literal `"DEFAULT"`, which `ListVersions` never publishes as a build identifier.
4. Expected (per `ChannelEngineVersionRequest$Version` and the identical branch in ops/update_channel.rs:356-366): the pin is reset, so the channel reports no pinned engine version — exactly as a channel created with no `channelEngineVersion` member does.
5. Contrast: CreateChannel without `channelEngineVersion`, then UpdateChannel with `channelEngineVersion: {"version": "1.3.0"}`, then UpdateChannel with `channelEngineVersion: {"version": "DEFAULT"}` — the update path clears the pin.
