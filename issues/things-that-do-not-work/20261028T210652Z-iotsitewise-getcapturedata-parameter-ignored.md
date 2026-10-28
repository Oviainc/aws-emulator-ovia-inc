Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# formatSettings is read off the request and discarded, so framesPerSecond/widthInPixels/heightInPixels change nothing and are reported nowhere

_Filed: 2026-10-28T21:06:52Z_
_Operation: GetCaptureData | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetCaptureDataRequest$formatSettings` carries the output format the caller asks for — `framesPerSecond` ("The target frame rate for the output"), `widthInPixels` ("The target width of the output, in pixels") and `heightInPixels` ("The target height of the output, in pixels"), under a shape documented as "Contains the output format configuration for video processing". The handler binds the member to `_` and never looks at it again: it neither influences which stored segment is selected, nor the bytes returned, nor any output member (the response models none that echoes it), nor is it refused. A caller that asks for 15 fps at 640x480 is answered 200 with the producer's original bytes and has no way to tell that its request was dropped.

The handler's own comment admits the gap ("this emulator serves the media exactly as the producer ingested it and runs no encoder, so the settings change neither which segments are selected nor the bytes returned"), so this is a documented-but-dropped input member rather than an unattempted feature: every other member of this request is either honoured (startTime/endTime/timeSeriesId/propertyAlias/nextToken) or refused when it cannot be, which is the discipline the rest of the handler states.

## Evidence

The model text the handler drops:
  smithy_query(query="shape", shape="FormatSettings") — "doc": "<p>Contains the output format configuration for video processing.</p>", members "framesPerSecond": "<p>The target frame rate for the output.</p>", "widthInPixels": "<p>The target width of the output, in pixels.</p>", "heightInPixels": "<p>The target height of the output, in pixels.</p>"
  and GetCaptureDataRequest$formatSettings — "<p>The optional format settings for the output.</p>"

The handler, dropping it:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_capture_data.rs:211-216
    "// `formatSettings` asks for the OUTPUT to be re-encoded (frame rate, width,
     // height). This emulator serves the media exactly as the producer ingested it
     // and runs no encoder, so the settings change neither which segments are
     // selected nor the bytes returned — a recorded limitation, not a refusal: the
     // request is one the real service accepts.
     let _ = &input.format_settings;"
  and the answer, which is the stored segment verbatim: src/ops/get_capture_data.rs:298-304 — "Ok(GetCaptureDataOutput { data: chunk.data.clone(), start_time: chunk.start_time.clone(), end_time: chunk.end_time.clone(), data_type: source.data_type.clone(), next_token, })"

The contrast inside the same handler — every other member is honoured or refused:
  src/ops/get_capture_data.rs:182-192 — "// `endTime`'s own documentation states the rule the model cannot: it \"must be greater than startTime\", so a range that covers no time is refused ... if end <= start { return Err(invalid_request(...)) }"
  src/ops/get_capture_data.rs:194-208 — the timeSeriesId/propertyAlias exclusivity refusal.

Test coverage: none. `grep -n "format_settings|formatSettings" tests/all/capture_data.rs` matches nothing, so no suite sends the member and nothing pins the current answer.

## Steps to Reproduce

1. CreateWorkspace "ws" and ingest capture data so GetCaptureData answers a segment for a known timeSeriesId.
2. POST /workspaces/ws/capture-data with {"timeSeriesId": "…", "startTime": {…}, "endTime": {…}} and note the `data` bytes returned.
3. Repeat the same call adding {"formatSettings": {"framesPerSecond": 1, "widthInPixels": 16, "heightInPixels": 16}}.
4. Observed: byte-for-byte the same `data`, the same span, and no member of the response referring to the requested format — the setting was accepted and silently discarded.
5. Nothing in the response or in an error tells the caller its requested output format was not applied.
