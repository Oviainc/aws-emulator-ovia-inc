Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-archived-media.get-images-no-fragments-not-found kinesis-video-archived-media.get-images-reversed-timestamp-range-rejected kinesis-video-archived-media.get-images-timestamp-window-exceeds-300-seconds-rejected kinesis-video-archived-media.get-images-width-height-pixels-accepted-on-empty-stream
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media.get-images-no-fragments-not-found|run#5|AssertionError|assert err == NO_FRAGMENTS, f"expected {NO_FRAGMENTS!r}, got {err!r}"
Issue-Key: kinesis-video-archived-media.get-images-reversed-timestamp-range-rejected|run#6|AssertionError|assert err == REVERSED, f"expected {REVERSED!r}, got {err!r}"
Issue-Key: kinesis-video-archived-media.get-images-timestamp-window-exceeds-300-seconds-rejected|run#5|AssertionError|assert err_wide == WINDOW_TOO_WIDE, (
Issue-Key: kinesis-video-archived-media.get-images-width-height-pixels-accepted-on-empty-stream|run#5|AssertionError|assert err == NO_FRAGMENTS, (
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Implement kinesis-video-archived-media GetImages: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImages

## Description
`GetImages` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/ops/get_images.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `GetImages` call, so each one unblocks only when `GetImages` works: `kinesis-video-archived-media.get-images-no-fragments-not-found`, `kinesis-video-archived-media.get-images-reversed-timestamp-range-rejected`, `kinesis-video-archived-media.get-images-timestamp-window-exceeds-300-seconds-rejected`, `kinesis-video-archived-media.get-images-width-height-pixels-accepted-on-empty-stream`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/ops/get_images.rs`. Replaying the corpus, the emulator refused the `GetImages` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_kinesis_video_archived_media_get_images_no_fragments_not_found.py", line 37, in run
    assert err == NO_FRAGMENTS, f"expected {NO_FRAGMENTS!r}, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.'), got ('NotImplemented', 'operation GetImages is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the kinesis-video-archived-media emulator
Call GetImages and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
