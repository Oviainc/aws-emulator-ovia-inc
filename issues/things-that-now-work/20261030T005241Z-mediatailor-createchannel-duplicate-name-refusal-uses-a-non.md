Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediatailor.create-channel-duplicate-name-rejected-without-mutating-existing
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor.create-channel-duplicate-name-rejected-without-mutating-existing|run#7|AssertionError|assert message == (
Issue-Commit: 1e71a3102b418a18abda1ed5ba2c04b37249c268

# mediatailor CreateChannel duplicate-name refusal uses a non-AWS BadRequestException message

> _Note: this issue was identified against the code base at commit `1e71a3102b418a18abda1ed5ba2c04b37249c268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: CreateChannel refuses a channel name that already exists with the right error shape and status but the wrong error message text.

AWS: Smithy model `services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json` — `CreateChannel` (POST /channel/{ChannelName}, success 200) declares NO operation-level errors, so the error text is recorded behavior only. The recorded AWS behavior for re-creating a channel under a name already in use is an HTTP 400 with `Error.Code` = `BadRequestException` and `Error.Message` = `This channel already exists. Create a unique channel and resubmit your request.` That message is entirely FIXED text — it has no variable parts: AWS does not interpolate the requested ChannelName, the ARN, or any other request/state value into it. The refusal is also side-effect-free: the already-stored channel keeps its original PlaybackMode, Tier and Outputs.

Emulator: the status (400) and the code (`BadRequestException`) match, but the message is a different, emulator-invented template that interpolates the request's ChannelName: `Channel with name <ChannelName> already exists.` (observed as `Channel with name sim2real-behav-mt-ch-16d56b195dbe already exists.`, where `sim2real-behav-mt-ch-16d56b195dbe` is the per-run name taken from the request and the rest is the emulator's fixed text). Because the check asserts the message before it can assert the no-mutation property, the rest of that check — that the rejected duplicate create leaves PlaybackMode=LOOP, Tier=BASIC and Outputs[0].ManifestName="default" untouched — is unverified by this run and must be re-checked once the message is corrected.

Why: clients and tooling branch on the error message for the duplicate-name case because the service models no distinct error shape for it (every client failure on this service surfaces as the single `BadRequestException`). A caller matching AWS's fixed text `This channel already exists. Create a unique channel and resubmit your request.` cannot distinguish a name collision from any other bad request against the emulator, so idempotent create-or-reuse logic and retry/backoff decisions take the wrong branch; test suites asserting AWS's wording fail against the emulator.

Fix: the duplicate-name refusal branch in the CreateChannel handler's own validation — it must emit AWS's fixed message with no interpolated ChannelName. The handler is the only file to touch for this operation. While fixing, re-assert that the refusal returns before any mutation so the existing channel record and its tag set are left unchanged.

Traces: mediatailor.create-channel-duplicate-name-rejected-without-mutating-existing (verdict: mismatch; AssertionError: unexpected message 'Channel with name sim2real-behav-mt-ch-16d56b195dbe already exists.'). Code citation for the diverging message: services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs lines 373-383.

## Evidence
Check: mediatailor.create-channel-duplicate-name-rejected-without-mutating-existing

Request sequence:
  1. CreateChannel(ChannelName=<name>, Outputs=[{ManifestName:"default", SourceGroup:"default", HlsPlaylistSettings:{ManifestWindowSeconds:30}}], PlaybackMode="LOOP", Tier="BASIC") -> 200
  2. CreateChannel(ChannelName=<same name>, Outputs=[{ManifestName:"other", SourceGroup:"other", HlsPlaylistSettings:{ManifestWindowSeconds:60}}], PlaybackMode="LOOP", Tier="BASIC")

Expected (AWS) on step 2:
  HTTP 400
  Error.Code    = "BadRequestException"
  Error.Message = "This channel already exists. Create a unique channel and resubmit your request."   <- fixed text, NO variable parts

Actual (emulator) on step 2:
  HTTP 400
  Error.Code    = "BadRequestException"            (matches)
  Error.Message = "Channel with name sim2real-behav-mt-ch-16d56b195dbe already exists."
                   ^ fixed text "Channel with name " + " already exists." around a value
                     interpolated from the request's ChannelName

Runner output:
  File "check_mediatailor_create_channel_duplicate_name_rejected_without_mutating_existing.py", line 58, in run
    assert message == (
  AssertionError: unexpected message 'Channel with name sim2real-behav-mt-ch-16d56b195dbe already exists.'

Source of the divergence (services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs:373-383):
  if store.channels.contains_key(&input.channel_name) {
      return Err(CreateChannelErrorModeled::BadRequestException(
          BadRequestException {
              message: Some(format!(
                  "Channel with name {} already exists.",
                  input.channel_name
              )),
          },
      )
      .into());
  }

Model evidence (smithy_query query="operation" operation="CreateChannel"):
  CreateChannel: http {method: POST, uri: "/channel/{ChannelName}", code: 200}, idempotent: true, errors: []
  -> no operation-level error is modeled; BadRequestException is the service's single
     widened-in client error, so only the message text distinguishes this refusal class.

Not asserted because the run stopped at the message assertion (re-verify after the fix):
  DescribeChannel(<name>) must still report PlaybackMode="LOOP", Tier="BASIC",
  Outputs[0].ManifestName="default".

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Create a channel:
   aws mediatailor create-channel --channel-name dup-demo \
     --outputs '[{"ManifestName":"default","SourceGroup":"default","HlsPlaylistSettings":{"ManifestWindowSeconds":30}}]' \
     --playback-mode LOOP --tier BASIC
3. Re-create the SAME name with different Outputs:
   aws mediatailor create-channel --channel-name dup-demo \
     --outputs '[{"ManifestName":"other","SourceGroup":"other","HlsPlaylistSettings":{"ManifestWindowSeconds":60}}]' \
     --playback-mode LOOP --tier BASIC
4. Observe the 400 BadRequestException message is 'Channel with name dup-demo already exists.' instead of AWS's fixed 'This channel already exists. Create a unique channel and resubmit your request.'
5. Then confirm the refusal did not mutate the stored channel:
   aws mediatailor describe-channel --channel-name dup-demo
   -> PlaybackMode must be LOOP, Tier must be BASIC, Outputs[0].ManifestName must be 'default'.
6. Re-replay via run_behavioral_suite: check mediatailor.create-channel-duplicate-name-rejected-without-mutating-existing must pass.
