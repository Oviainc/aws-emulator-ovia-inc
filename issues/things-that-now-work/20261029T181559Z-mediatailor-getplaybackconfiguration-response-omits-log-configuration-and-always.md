Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-playback-configuration-log-configuration
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-playback-configuration-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.LiveAdBuffetMode
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.LogConfiguration.EnabledLoggingStrategies[*]
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.LogConfiguration.PercentEnabled
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.VodAdBuffetMode
Issue-Key: mediatailor|GetPlaybackConfiguration|2xx->2xx|$.tags
Issue-Commit: fad1fdecc61c47880f0bf921597b8afa67a7a57d

# mediatailor GetPlaybackConfiguration response omits log configuration and always-present members

> _Note: this issue was identified against the code base at commit `fad1fdecc61c47880f0bf921597b8afa67a7a57d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlaybackConfiguration

## Description
Cause 1 of 1: GetPlaybackConfiguration returns HTTP 200 but its response projection drops the playback configuration's logging state and several members AWS always emits. Fixture mediatailor-playback-configuration-log-configuration, step task#3.

AWS: GetPlaybackConfiguration is GET /playbackConfiguration/{Name} -> 200 with GetPlaybackConfigurationResponse (Smithy: com.amazonaws.mediatailor#GetPlaybackConfigurationResponse). Per the model it projects the whole stored configuration, including:
  * LogConfiguration (target com.amazonaws.mediatailor#LogConfiguration), whose members PercentEnabled (@default 0, required) and EnabledLoggingStrategies (list of LoggingStrategy, values VENDED_LOGS | LEGACY_CLOUDWATCH, clientOptional+required) hold whatever was last set for that playback configuration. The log settings are written by ConfigureLogsForPlaybackConfiguration (PUT /configureLogs/playbackConfiguration, input PercentEnabled required + EnabledLoggingStrategies), so a Get after a configure-logs call must read back the values that call stored, not the structure's defaults.
  * Tags, which is serialized under the wire key "tags" (Smithy: GetPlaybackConfigurationResponse$Tags @jsonName("tags")). AWS emits the key with an empty object when the configuration carries no tags.
  * LiveAdBuffetMode and VodAdBuffetMode: recorded behavior only - these top-level members are not present in this repo's on-disk mediatailor model, and the recording shows AWS returning them on every GetPlaybackConfiguration response with the value "INSERT_ALL" for a configuration that never set them (i.e. they are server-defaulted, always-present members).

Emulator: the step returns 200 with a body in which
  * $.LogConfiguration.PercentEnabled is 0 instead of the 50 that was configured for this playback configuration,
  * $.LogConfiguration.EnabledLoggingStrategies is empty - index [0] is absent where VENDED_LOGS is expected,
  * $.tags is absent entirely rather than an empty map,
  * $.LiveAdBuffetMode is absent rather than INSERT_ALL,
  * $.VodAdBuffetMode is absent rather than INSERT_ALL.
Status and shape are otherwise right, so this is a body-fidelity defect, not a routing or error-mapping one. Note the sibling open issue 20261029T174215Z-mediatailor-configurelogsforplaybackconfiguration-stub.md (ConfigureLogsForPlaybackConfiguration is an unimplemented scaffold stub); that issue covers the configure-logs operation itself and does not cover GetPlaybackConfiguration's own projection, which must read the stored log settings back and must emit the always-present members above regardless of how they were written.

Why: a client that reads a playback configuration back to learn its logging posture sees PercentEnabled 0 and no enabled logging strategies, i.e. "session logging is off", for a configuration where logging is on at a nonzero percentage with VENDED_LOGS - so SDK callers, console-style read-after-write flows and infrastructure tools that reconcile desired vs actual log settings will conclude the configure-logs call never took effect and will either loop re-applying it or report drift. The missing "tags" key makes tag-reading clients dereference an absent map instead of an empty one, and the missing LiveAdBuffetMode/VodAdBuffetMode members make typed clients see null for members AWS never omits, so code that switches on the ad-buffet mode gets no value at all.

Fix: GetPlaybackConfiguration's handler and response serializer in the mediatailor service crate (services/service-com-amazonaws-mediatailor-mediatailor), plus the playback-configuration state it reads. The handler must (a) project the LogConfiguration actually stored for the named configuration - PercentEnabled and EnabledLoggingStrategies as last written by ConfigureLogsForPlaybackConfiguration - rather than emitting a freshly defaulted LogConfiguration; (b) always serialize Tags under the jsonName "tags", as {} when the configuration has no tags; and (c) always emit the server-defaulted LiveAdBuffetMode and VodAdBuffetMode members. This also implies the playback-configuration state record must carry the log settings so they survive the write-then-read sequence.

All values above are described as rules: the member names and the wire key "tags" are fixed; PercentEnabled and EnabledLoggingStrategies are whatever the request that configured logs supplied for that configuration (50 and [VENDED_LOGS] in this recording, not a constant); the ad-buffet modes are the server default INSERT_ALL.

## Evidence
Fixture mediatailor-playback-configuration-log-configuration, step task#3, operation GetPlaybackConfiguration, verdict mismatch, expected_status 200, actual_status 200.

Replay detail (expected = recorded AWS, got = emulator):
  $.tags: missing (expected {}, got None)
  $.LogConfiguration.PercentEnabled: value_mismatch (expected 50, got 0)
  $.LogConfiguration.EnabledLoggingStrategies[0]: missing (expected 'VENDED_LOGS', got None)
  $.LiveAdBuffetMode: missing (expected 'INSERT_ALL', got None)
  $.VodAdBuffetMode: missing (expected 'INSERT_ALL', got None)

divergence_key: service=mediatailor|GetPlaybackConfiguration|status=200->200|keys=$.LiveAdBuffetMode,$.LogConfiguration.EnabledLoggingStrategies[0],$.LogConfiguration.PercentEnabled,$.VodAdBuffetMode,$.tags

Model evidence: Smithy GetPlaybackConfiguration -> GET /playbackConfiguration/{Name}, code 200, output GetPlaybackConfigurationResponse; GetPlaybackConfigurationResponse$Tags @jsonName("tags"); GetPlaybackConfigurationResponse$LogConfiguration -> LogConfiguration{PercentEnabled: __integer @default 0 required; EnabledLoggingStrategies: __listOfLoggingStrategies required}; LoggingStrategy enum values = [VENDED_LOGS, LEGACY_CLOUDWATCH]; ConfigureLogsForPlaybackConfiguration -> PUT /configureLogs/playbackConfiguration with required PercentEnabled and PlaybackConfigurationName plus EnabledLoggingStrategies. LiveAdBuffetMode / VodAdBuffetMode are absent from the on-disk model: recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture mediatailor-playback-configuration-log-configuration and stop at step task#3 (GetPlaybackConfiguration).
   Equivalently, by hand against the emulator's mediatailor endpoint:
   a. PUT /playbackConfiguration creating a playback configuration by Name (no Tags supplied).
   b. PUT /configureLogs/playbackConfiguration with {"PlaybackConfigurationName": "<that name>", "PercentEnabled": 50, "EnabledLoggingStrategies": ["VENDED_LOGS"]}.
   c. GET /playbackConfiguration/<that name>.
3. Inspect the 200 body from step (c). Observed: LogConfiguration.PercentEnabled = 0, LogConfiguration.EnabledLoggingStrategies = [] (no VENDED_LOGS entry), and no "tags", "LiveAdBuffetMode" or "VodAdBuffetMode" keys at all.
4. Expected: LogConfiguration.PercentEnabled = the value configured in (b) (50 here), LogConfiguration.EnabledLoggingStrategies = the strategies configured in (b) (["VENDED_LOGS"] here), "tags": {} present for an untagged configuration, and LiveAdBuffetMode = VodAdBuffetMode = "INSERT_ALL".
