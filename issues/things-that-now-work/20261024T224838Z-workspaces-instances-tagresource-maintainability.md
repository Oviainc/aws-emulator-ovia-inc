Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6

# The Tag-list-to-map merge rule is implemented three times in this crate

_Filed: 2026-10-24T22:48:38Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that turns a modelled `TagList` into the crate's stored `BTreeMap<String, String>` — an entry with no `Key` names no tag and is skipped, an absent `Value` becomes the empty string, and a repeated key keeps the last value — is written out three separate times in this crate's handlers: `TagResource`, `CreateWorkspaceInstance` and `CreateVolume`. The three copies can drift (one of them learning to trim, or to reject a keyless entry, while the others do not), and the crate already keeps the INVERSE direction in one place (`crate::state::tag_list`). Extracting one helper (e.g. `state::merge_tags(&mut BTreeMap, impl IntoIterator<Item = Tag>)`) leaves every response and every existing test unchanged.

## Evidence

Copy 1 — services/.../src/ops/tag_resource.rs:180-185:
```
        let mut merged = record.tags.clone();
        for tag in input.tags {
            if let Some(key) = tag.key {
                merged.insert(key, tag.value.unwrap_or_default());
            }
        }
```
Copy 2 — services/.../src/ops/create_workspace_instance.rs:259-264:
```
        let mut tags = std::collections::BTreeMap::new();
        for tag in input.tags.unwrap_or_default() {
            if let Some(key) = tag.key {
                tags.insert(key, tag.value.unwrap_or_default());
            }
        }
```
Copy 3 — services/.../src/ops/create_volume.rs:412-416:
```
            for tag in specification.tags.unwrap_or_default() {
                if let Some(key) = tag.key {
                    tags.insert(key, tag.value.unwrap_or_default());
                }
            }
```
Each copy restates the same rule in its comment: tag_resource.rs:172-175 "Tagging MERGES onto what is already stored, and a key already present takes the new value. `Tag.Key` and `Tag.Value` are both optional in the model; an entry with no key names no tag, and an absent value is the empty value its `len 0..256` allows."; create_workspace_instance.rs:253-254 "a repeated key keeps the last value, which is what merging onto a map gives."; create_volume.rs:401-402 "A repeated key keeps the last value, which is what merging onto a map gives." The opposite direction is already centralised — state.rs:2177 `pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag>`.

Tests that pin the behaviour and must stay green unchanged: tests/all/workspace_instance_lifecycle.rs:683-706 (merge cases: "a new key", "the same key again", "an empty value", "several keys at once", "an empty list"), tests/all/workspace_instance_lifecycle.rs:710 `the_stored_tag_set_cannot_grow_past_what_the_model_can_report`, tests/all/reserved_tag_keys.rs:98 `tag_resource_refuses_a_reserved_key_and_leaves_the_tag_set_unchanged`, tests/all/regional_scoping.rs:310, tests/all/volume_lifecycle.rs.

## Steps to Reproduce

1. Open src/ops/tag_resource.rs:180-185, src/ops/create_workspace_instance.rs:259-264 and src/ops/create_volume.rs:412-416 side by side and compare the three `if let Some(key) = tag.key { map.insert(key, tag.value.unwrap_or_default()); }` loops.
2. Note that src/state.rs:2177 already centralises the inverse conversion (`tag_list`), so the crate has a home for the merge direction too.
3. Change the rule in one copy only (e.g. skip an empty-string key) and observe the other two paths diverge while the suites above still pass.
