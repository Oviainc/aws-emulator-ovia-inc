Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# ResetChannelState resets a MULTIVIEW channel instead of refusing it

_Filed: 2026-10-24T07:16:22Z_
_Operation: ResetChannelState | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MULTIVIEW channel receives no ingest of its own — the model's InputType doc says "The channel receives no ingest of its own. Instead, it composites video from the source channels in its MultiviewConfiguration" — so there is no ingest state to discard, and the model carries a dedicated discriminator for the refusal: ValidationExceptionType MULTIVIEW_RESET_NOT_ALLOWED. ResetChannelState declares ValidationException (400).

Expected: ResetChannelState on a channel whose InputType is MULTIVIEW is refused with 400 ValidationException / MULTIVIEW_RESET_NOT_ALLOWED and no ResetAt is recorded. Actual: the handler resolves the channel and unconditionally stamps channel.reset_at = now(), answering 200 with a ResetAt that a later GetChannel reports — so a multiview channel claims to have had an ingest stream reset. The handler never looks at channel.input_type / channel.multiview_configuration, even though PutChannelPolicy already uses exactly that predicate for its own multiview refusal (MULTIVIEW_CHANNEL_POLICY_NOT_ALLOWED).

## Evidence

Spec: ValidationExceptionType includes MULTIVIEW_RESET_NOT_ALLOWED; ResetChannelState declares ValidationException (400); InputType doc states a MULTIVIEW channel receives no ingest of its own.
Handler: src/ops/reset_channel_state.rs lines 187-205 — channel lookup then `channel.reset_at = Some(reset_at)` with no input-type check; compare src/ops/put_channel_policy.rs lines 221-230, which refuses a multiview channel using channel.multiview_configuration.is_some().
Tests: tests/all/channel_reset.rs reset_cases() (lines 38-69) covers only absent/HLS/CMAF input types and asserts success; no multiview row and no refusal assertion.

## Steps to Reproduce

1. CreateChannelGroup g; CreateChannel g/src with InputType CMAF.
2. CreateChannel g/tiled with InputType MULTIVIEW and MultiviewConfiguration {AvailableSources: ['src'], AvailableLayouts: ['LAYOUT_2EH']}.
3. POST /channelGroup/g/channel/tiled/reset.
4. Observe 200 with a ResetAt (and GetChannel g/tiled reporting it); AWS answers 400 ValidationException with ValidationExceptionType MULTIVIEW_RESET_NOT_ALLOWED and records no reset.
