Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateChannel accepts a MULTIVIEW channel with no MultiviewConfiguration and never validates its available sources or layouts

_Filed: 2026-10-24T07:16:22Z_
_Operation: CreateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The input-type rule table (crate::state::CHANNEL_INPUT_TYPE_RULES / channel_input_type_violation) only checks the "present but not allowed" direction: a member is refused when it is submitted on an input type that may not carry it. The opposite direction and the contents of MultiviewConfiguration are unchecked, so several modeled refusals never happen:

- MULTIVIEW_CONFIGURATION_REQUIRED: MultiviewConfiguration's own doc says "This setting is required when InputType is MULTIVIEW, and can't be set for any other input type." CreateChannel with InputType=MULTIVIEW and no MultiviewConfiguration is accepted with 200, creating a multiview channel that composites nothing. AWS answers 400 ValidationException.
- MULTIVIEW_SOURCE_NOT_FOUND / MULTIVIEW_SOURCE_INVALID_INPUT_TYPE: "Each source channel must be in the same channel group as the multiview channel, and must have an InputType of CMAF." AvailableSources is stored without resolving a single name against group.channels, so a multiview channel can name channels that do not exist, or HLS-input channels, and still be created.
- MULTIVIEW_DUPLICATE_SOURCE / MULTIVIEW_DUPLICATE_LAYOUT: "Each layout must appear at most once"; repeated entries in AvailableSources or AvailableLayouts are accepted and stored.

UpdateChannel shares the same rule table (src/ops/update_channel.rs lines 293-317) and therefore the same blind spots: an update can repoint a multiview channel at a non-existent or non-CMAF source.

## Evidence

Spec: MultiviewConfiguration doc ("required when InputType is MULTIVIEW"; "Each source channel must be in the same channel group ... and must have an InputType of CMAF"; "Each layout must appear at most once"); ValidationExceptionType includes MULTIVIEW_CONFIGURATION_REQUIRED, MULTIVIEW_SOURCE_NOT_FOUND, MULTIVIEW_SOURCE_INVALID_INPUT_TYPE, MULTIVIEW_DUPLICATE_SOURCE, MULTIVIEW_DUPLICATE_LAYOUT; CreateChannel declares ValidationException (400).
Handler/state: src/state.rs channel_input_type_violation (lines 2196-2217) is `rules.find(|r| (r.present)(&submitted) && !r.allowed.contains(&input_type))` — presence-only, no required-member and no source/layout resolution; src/ops/create_channel.rs line 345 is the only cross-member check and line 448 stores input.multiview_configuration unexamined.
Tests: tests/all/update_channel.rs lines 511-517 state outright that "the source rules themselves — MULTIVIEW_SOURCE_NOT_FOUND / MULTIVIEW_SOURCE_INVALID_INPUT_TYPE — are enforced by neither create nor update yet", and the rows deliberately use valid sources, so nothing asserts these refusals.

## Steps to Reproduce

1. CreateChannelGroup g.
2. CreateChannel g/tiled1 with InputType MULTIVIEW and no MultiviewConfiguration. Observe 200; AWS answers 400 ValidationException / MULTIVIEW_CONFIGURATION_REQUIRED.
3. CreateChannel g/plain with InputType HLS, then CreateChannel g/tiled2 with InputType MULTIVIEW and MultiviewConfiguration {AvailableSources: ['plain','does-not-exist','plain'], AvailableLayouts: ['LAYOUT_2EH','LAYOUT_2EH']}. Observe 200 and GetChannel echoing all of it; AWS answers 400 ValidationException (MULTIVIEW_SOURCE_NOT_FOUND / MULTIVIEW_SOURCE_INVALID_INPUT_TYPE / MULTIVIEW_DUPLICATE_SOURCE / MULTIVIEW_DUPLICATE_LAYOUT).
4. Repeat the second case through UpdateChannel on an existing MULTIVIEW channel — same acceptance.
