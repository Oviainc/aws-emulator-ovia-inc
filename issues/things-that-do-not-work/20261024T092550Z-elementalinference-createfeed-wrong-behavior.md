Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 19809ff85dacb8a5011e45ecfee3e76273a4f2a9

# Documented output-config defaults are not materialised: contextualMetadata summaryGeneration/extendedAnalysis and subtitling profanityFilter come back absent

_Filed: 2026-10-24T09:25:50Z_
_Operation: CreateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `19809ff85dacb8a5011e45ecfee3e76273a4f2a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator stores an output's config with the service's own resolved defaults materialised (`crate::state::output_config_with_defaults`), but it materialises exactly ONE of them — a subtitling output's `aspectRatio` (16:9). The model documents further members the service resolves for itself when the request omits them:

* `ContextualMetadataConfig$summaryGeneration`: "ENABLED (default) - Elemental Inference populates the summary, objects, and actions fields..."
* `ContextualMetadataConfig$extendedAnalysis`: "ENABLED (default) - Elemental Inference populates the people, environments, brands, and on-screen text fields..."
* `SubtitlingConfig$profanityFilter`: "Valid values: DISABLED (no filtering, default), CENSOR..., DROP..."

A feed created with `{"contextualMetadata": {}}` (or with only one of the two members) or with a subtitling config carrying no `profanityFilter` is therefore stored and reported with those members ABSENT, where the real service reports the configuration it actually applied (`ENABLED`/`ENABLED`, `DISABLED`). The same omission then propagates to `GetFeed`, `UpdateFeed` and `AssociateFeed`-appended outputs, and it breaks the read-modify-write round trip the handler's own comment says the defaults exist for: a client that reads a feed with GetFeed and PUTs it straight back with UpdateFeed (a full replacement) carries back a config in which the service's resolved analysis settings were never stated.

Expected: the created/stored output config reports the documented defaults for the members the request omitted. Actual: only `subtitling.aspectRatio` is filled in; the contextual-metadata modes and the profanity filter stay absent.

## Evidence

Spec: `smithy_query(query="shape",shape="ContextualMetadataConfig")` - both members' docs name "ENABLED (default)"; `SubtitlingConfig$profanityFilter` doc names "DISABLED (no filtering, default)".
Handler: `src/state.rs` `output_config_with_defaults()` only touches `OutputConfig::Subtitling.aspect_ratio` and its doc comment asserts "the cropping, clipping and contextual-metadata configs declare no member the service resolves", which contradicts the ContextualMetadataConfig docs above; `src/ops/create_feed.rs` / `update_feed.rs` / `associate_feed.rs` all build stored outputs through `crate::state::Output::new`, which calls that function.
Tests: `tests/all/response_members.rs:296-346` asserts the CURRENT behavior - the row "contextualMetadata, which models no service-resolved member" expects the submitted config echoed verbatim, and the "subtitling without an aspect ratio" row expects no `profanityFilter` in the response - so this divergence is locked in by a test rather than covered by one.

## Steps to Reproduce

1. POST /v1/feed with outputs [{"name":"meta1","outputConfig":{"contextualMetadata":{}},"status":"ENABLED"}] -> observe the response output config as `{"contextualMetadata": {}}` instead of carrying summaryGeneration=ENABLED and extendedAnalysis=ENABLED
2. GET /v1/feed/{id} -> the stored output config still omits both members
3. POST /v1/feed with outputs [{"name":"subs1","outputConfig":{"subtitling":{"language":"eng"}},"status":"ENABLED"}] -> observe aspectRatio filled in as 16:9 but `profanityFilter` absent instead of DISABLED
