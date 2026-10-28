Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The "empty tag map reports no Tags member" rule is implemented twice, once in state.rs and once in ListTagsForResource

_Filed: 2026-10-28T05:28:10Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides how a stored environment's tag map becomes a modelled `Tags` member — omit the member when the map is empty, otherwise convert the record's `BTreeMap<String, String>` into the generated `HashMap<String, String>` — is written out twice in this crate, with the same justifying comment copied verbatim above each copy.

`EnvironmentRecord::to_environment` (the shape `GetEnvironment` returns) applies the rule through the crate's own conversion helper `map_of` (state.rs:1060). `ListTagsForResource`'s handler applies the SAME rule but hand-rolls the conversion inline instead of calling that helper, even though the helper is in the module it already imports for every other value it reads.

Two copies of one rule can drift: a change to how an empty map, or how the map's key/value cloning, is reported in one response would silently leave the other response answering the old way, and the two are required to agree (an existing test asserts exactly that `ListTagsForResource` and `GetEnvironment` agree an environment is untagged). The fix is for the `ListTagsForResource` handler to go through the same single place `to_environment` uses (e.g. a `pub` accessor on `EnvironmentRecord` wrapping the existing `map_of` + emptiness test) so the rule is decided once. Every response byte and every existing test stays exactly as it is.

## Evidence

Copy 1 — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:873-875, inside `EnvironmentRecord::to_environment`:
```rust
            // `TagMap` is `@length(1, 50)`, so an untagged environment reports no `Tags`
            // member rather than an empty map outside that bound.
            tags: (!self.tags.is_empty()).then(|| map_of(&self.tags)),
```
where the helper it calls is services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:1059-1065:
```rust
/// A `BTreeMap` as the `HashMap` every modelled map member is generated as.
fn map_of(source: &BTreeMap<String, String>) -> HashMap<String, String> {
    source
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
```

Copy 2 — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/list_tags_for_resource.rs:164-173, the identical comment over an inline re-implementation of `map_of`:
```rust
        // `TagMap` is `@length(1, 50)`, so an untagged environment reports no `Tags`
        // member rather than an empty map outside that bound.
        Ok(ListTagsForResourceOutput {
            tags: (!stored.tags.is_empty()).then(|| {
                stored
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        })
```

Test coverage that pins both copies and must stay unchanged — services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/environment_lifecycle.rs:1231-1250:
```rust
        // Emptying: `TagMap` is @length(1, 50), so no `Tags` member is reported at all.
...
            harness.list_tags(&arn).await.json().get("Tags").cloned(),
            None,
            "case {}: an untagged environment reports no Tags member",
...
            harness.environment(case.name).await.get("Tags").cloned(),
            None,
            "case {}: GetEnvironment agrees that the environment is untagged",
```
plus environment_lifecycle.rs:1183-1226 (create-seeded tags and `TagResource`/`UntagResource` round-trips read back through `ListTagsForResource` and `GetEnvironment`), tests/all/reserved_tag_keys.rs:168-180 and tests/all/environment_teardown_write_gate.rs:257-269, which read the same member through the handler copy.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs at line 873 and read the comment plus `tags: (!self.tags.is_empty()).then(|| map_of(&self.tags))`.
2. Open services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/list_tags_for_resource.rs at line 164 and read the same comment over an inline copy of `map_of`'s body, instead of a call to the helper at state.rs:1060.
3. Observe that changing the rule (the emptiness test or the map conversion) in either place leaves the other response answering the old way, while environment_lifecycle.rs:1239-1250 requires the two responses to agree.
4. Run the crate's tests (`environment_lifecycle`, `reserved_tag_keys`, `environment_teardown_write_gate`) before and after routing the handler through the single shared place: every assertion and every response must be unchanged.
