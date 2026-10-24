Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# DeleteChannel deletes a channel that a multiview channel still lists as an available source

_Filed: 2026-10-24T07:16:22Z_
_Operation: DeleteChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states the rule explicitly on GetChannelResponse$AttachedMultiviewChannels: "The multiview channels, in the same channel group, that list this channel as an available source. This is a read-only field. You can't delete a channel while any multiview channel still lists it as a source. Use this field to find the multiview channels that you need to update first." DeleteChannel declares ConflictException (409), and the emulator already uses that error for the sibling containment rule (a channel group with channels, a channel with origin endpoints, via crate::state::children_attached / RESOURCE_IN_USE).

Expected: DeleteChannel on a channel whose name appears in another channel's MultiviewConfiguration.AvailableSources is refused with 409 ConflictException (ConflictExceptionType RESOURCE_IN_USE) and the channel survives. Actual: the handler checks only that the channel has no origin endpoints, then removes it; the multiview channel is left pointing at a source that no longer exists, and GetChannel on the multiview channel still reports the dangling source while the deleted channel 404s. The emulator already computes the exact predicate needed for the refusal — crate::state::attached_multiview_channels(group, name), used by GetChannel/ListChannels — but DeleteChannel does not consult it.

## Evidence

Spec: GetChannelResponse$AttachedMultiviewChannels documentation ("You can't delete a channel while any multiview channel still lists it as a source"); DeleteChannel errors include ConflictException httpError 409; ConflictExceptionType includes RESOURCE_IN_USE.
Handler: src/ops/delete_channel.rs lines 165-186 — resolves group and channel, refuses only on non-empty channel.origin_endpoints, then group.channels.remove(..). No call to crate::state::attached_multiview_channels (defined in src/state.rs ~1591).
Tests: tests/all/channel_listing.rs asserts AttachedMultiviewChannels is reported (lines 338-406) but no test anywhere asserts the delete refusal; grep of tests/ for AttachedMultiviewChannels returns only that read assertion.

## Steps to Reproduce

1. CreateChannelGroup g.
2. CreateChannel g/source with InputType CMAF.
3. CreateChannel g/tiled with InputType MULTIVIEW and MultiviewConfiguration {AvailableSources: ['source'], AvailableLayouts: ['LAYOUT_2EH']}.
4. GetChannel g/source and confirm AttachedMultiviewChannels contains 'tiled'.
5. DeleteChannel g/source. Observe 200 and the channel gone; AWS answers 409 ConflictException (RESOURCE_IN_USE) and keeps the channel until 'tiled' stops naming it.
