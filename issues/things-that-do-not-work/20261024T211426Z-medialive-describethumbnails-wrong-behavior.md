Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# DescribeThumbnails always answers an empty thumbnailDetails list: a RUNNING channel with ThumbnailConfiguration AUTO never reports a thumbnail

_Filed: 2026-10-24T21:14:26Z_
_Operation: DescribeThumbnails | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeThumbnails` is documented "Describe the latest thumbnails data" and `ThumbnailDetail` is "Thumbnail details for one pipeline of a running channel". For a channel that is RUNNING with `EncoderSettings.ThumbnailConfiguration.State = AUTO`, the real service answers with one `ThumbnailDetail` per queried pipeline carrying a `Thumbnail` whose `Body` is base64 image data, a `ContentType` and a `TimeStamp`.

The emulator's thumbnail log is only ever written by `ComAmazonawsMedialiveMedialiveState::capture_channel_thumbnail`, and NO operation in the crate calls it, so the log is permanently empty for every channel. `DescribeThumbnails` therefore always answers 200 with `thumbnailDetails: []`, whatever the channel's state and whatever its thumbnail configuration says. A client that polls thumbnails (the console preview flow, or any health check that asserts a frame arrives after the channel starts) can never observe one.

Relatedly, the operation models `ConflictException` (409) — a state-dependent refusal the handler never produces: a channel that is not RUNNING, or whose thumbnail feature is off, is answered with an empty 200 rather than a refusal.

## Evidence

`src/state/thumbnails.rs` module docs lines 22-31: "NOTHING in this emulator captures a frame… a channel created here carries an empty capture log"; `capture_channel_thumbnail` is defined at line 204 and `grep` over the crate shows it is referenced only from doc comments (`src/state.rs:896`, `src/ops/describe_thumbnails.rs:7`) — never called. `ops/describe_thumbnails.rs` lines 200-208 build `thumbnail_details` from `channel.latest_thumbnail(...)`, which is always `None`. Spec grounding: `smithy_query(query="operation", operation="DescribeThumbnails")` models `ThumbnailDetails` as the whole output plus a `ConflictException` (409); `ThumbnailConfiguration$State` documents "AUTO turns the feature on" and that the feature "generates thumbnails of the incoming video in each pipeline in the channel". No test asserts a non-empty response.

## Steps to Reproduce

1. CreateChannel with EncoderSettings.ThumbnailConfiguration.State = AUTO and ChannelClass = SINGLE_PIPELINE
2. StartChannel and wait for DescribeChannel to report state RUNNING
3. GET /prod/channels/{id}/thumbnails?pipelineId=0&thumbnailType=CURRENT_ACTIVE
4. Observe 200 with thumbnailDetails: [] (AWS returns one ThumbnailDetail with a Thumbnail carrying Body/ContentType/TimeStamp)
5. StopChannel and repeat the same call — observe the same empty 200 rather than a state-based refusal
