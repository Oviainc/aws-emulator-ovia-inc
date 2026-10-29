Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae9a88928ee39d3ca8574060b3b992fb0f13001f

# TagResource writes an agent profile a tag value its own modelled Tag$value cannot carry, so GetAgentProfile reports a tag outside the modelled constraints

_Filed: 2026-10-29T00:07:43Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ae9a88928ee39d3ca8574060b3b992fb0f13001f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An agent profile is one of the resources this crate's single tagging plane resolves (`TaggedResource::AgentProfile`), so `TagResource` writes straight into the agent profile's stored tag map. But the two tag shapes of this service are NOT interchangeable: `TagResource` takes a `TagMap`, whose value (`TagValue`) is `@length(max: 256)` with no minimum and no pattern, while an agent profile's own tags are a `Tags` LIST of `Tag`, whose `value` is `@required` with `@length(min: 1, max: 256)` AND `@pattern("^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$")`.

`TagResource` checks only the tag COUNT before `stored.extend(tags)`, so a value the agent profile's tag shape cannot carry — the empty string, or any value containing a character outside `Tag$value`'s pattern such as a comma or `%` — is accepted and persisted on the agent profile. Every later read of that profile projects the stored map through `ProfileRecord::tag_list()` into `Tag { key, value }`, so `GetAgentProfile` / `ListAgentProfiles` answer 200 with a `Tag` whose `value` is empty (or pattern-violating): a value the model says that member cannot hold, and which the crate's own test suite states the agent profile's tag plane refuses.

The crate already knows the rule — it states it in `tests/all/resource_tags.rs:100-108` to explain why the create path cannot take such a row — but only the boundary validator of `CreateAgentProfile` enforces it; the `TagMap` door onto the same resource enforces nothing. Nothing in `crate::state`'s tag helpers (`merged_tag_count`, `tag_ceiling_constraint`) looks at a value at all.

## Evidence

Model, the two tag shapes (smithy_query query="shape" shape="Tag,TagMap"):
- `"Tag": {"members": {"key": {"length": {"min": 1, "max": 128}, "pattern": "^[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]+$", "required": true}, "value": {"length": {"min": 1, "max": 256}, "pattern": "^[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]+$", "required": true}}}`
- `"TagMap": {"length": {"min": 1, "max": 50}, "key": {"target": "TagKey", ...}, "value": {"target": "TagValue", "length": {"max": 256}}}` — no minimum, no pattern.

Side 1 — the crate states the rule itself: `tests/all/resource_tags.rs:100-108` — "Whether a create of this kind can record the tags of a `tag_map_only` row. An agent profile's tags are a `Tags` LIST of `Tag`, whose `value` is `@length(1, 256)` with a pattern (`smithy_query(query=\"shape\", shape=\"Tag\")`), where a `TagMap` value is `@length(max: 256)` with no pattern and no minimum — so an empty value is a tag only a `TagMap` carries, and boundary validation refuses it for an agent profile before any handler runs." / `pub(crate) fn carries_tag_map_only_rows(self) -> bool { self != Kind::AgentProfile }`

Side 2 — the write path applies no value rule and lands on exactly that resource: `src/ops/tag_resource.rs:149` — `let tags: std::collections::BTreeMap<String, String> = input.tags.into_iter().collect();`; `:151-158` — `let stored = store.resource_tags_mut(&input.workload_arn).ok_or_else(...)`; the only check is on the count, `:166` — `if crate::state::merged_tag_count(stored, &tags) > crate::state::MAX_TAGS_PER_RESOURCE {`; then `:180-182` — "'Adds one or more tags to the specified resource': the request's pairs are added to the ones the resource carries, a repeated key taking the new value." / `stored.extend(tags);`

The resolution really reaches the agent profile: `src/state.rs:6639-6641` — `if self.agent_profiles.contains_key(resource_arn) { return Some(TaggedResource::AgentProfile); }`, and `src/state.rs:6613-6616` — `TaggedResource::AgentProfile => self.agent_profiles.get_mut(resource_arn).map(|record| &mut record.tags),`.

What the read then reports: `src/state.rs:5239-5251` — "The profile's tags as the wire carries them … the modelled `Tags` member is a list of `Tag` key/value pairs" / `.map(|(key, value)| Tag { key: key.clone(), value: value.clone() })`, reached from `src/ops/get_agent_profile.rs:208` — `tags: record.tag_list(),`.

Reachability (and the one test row a fix must narrow): `tests/all/resource_tagging.rs:138-143` — `const KINDS: [Kind; 5] = [ … Kind::AgentProfile, ];`, whose rule-1 test `:315-342` tags every kind with the rows at `:257-279`, including `("an empty value, which TagValue's @length has no minimum for".to_string(), tags(&[("marker", "")]))` and `("every character class the TagKey pattern admits".to_string(), tags(&[("Ünïcode Lëttérs 09_.:/=+@-", "a value with spaces, punctuation and =+@-/ in it")]))` — that second value contains a comma, which `Tag$value`'s pattern forbids. Both rows assert only through `ListTagsForResource` (the `TagMap` plane), so neither pins what `GetAgentProfile` reports for the agent-profile case; the stated reason for those rows is `TagValue`'s own bounds, which say nothing about the agent profile's `Tag`.

## Steps to Reproduce

1. POST /api/v1/agent-profiles (CreateAgentProfile) with a valid name, pillars, executionRoleArn and aggregationConfiguration; note the returned `arn`
2. POST /tags/<that agent-profile ARN> (TagResource) with {"Tags": {"marker": ""}} — the empty value satisfies TagMap/TagValue, so the boundary accepts it and the handler stores it (src/ops/tag_resource.rs:149, :182)
3. GET /api/v1/agent-profiles/<arn> (GetAgentProfile): the response's `tags` contains {"key": "marker", "value": ""} — a `Tag` whose `value` the model declares @required with @length(min: 1) and a pattern (src/state.rs:5246-5249, src/ops/get_agent_profile.rs:208)
4. Repeat with {"Tags": {"marker": "a,b"}}: the comma passes TagValue (no pattern) and is reported as a `Tag$value` the modelled pattern forbids
5. Compare with CreateAgentProfile carrying the same pair as a `Tags` list entry, which the boundary refuses 400 — the rule tests/all/resource_tags.rs:100-108 states for this resource
