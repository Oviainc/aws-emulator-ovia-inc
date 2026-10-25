Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# The modelled TagKey length/pattern rules are stated twice in state.rs, so the two copies can drift

_Filed: 2026-10-25T00:18:41Z_
_Operation: UntagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` states the same `TagKey` rules (length 1..128 and pattern `^[a-zA-Z0-9 _.:/=+@-]+$`) in two independent places: once as the `TAG_KEY_MIN_LEN` / `TAG_KEY_MAX_LEN` / `TAG_KEY_PATTERN` constants that the tag-map key check is built from, and once again as hard-coded literals in the `tagKeys` entry of `LIST_ELEMENT_CONSTRAINTS`, which is what the validation report composes for a failing `UntagResource` tag key. Only the first copy is pinned to the model by a test; the second repeats the same three values as literals, so updating one (e.g. if the model's `TagKey` pattern changes) silently leaves the other stating a different rule and `UntagResource`'s refusal message would then describe constraints the handler does not enforce. The `tagKeys` entry can be expressed in terms of the existing constants, leaving every response byte and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:1844-1858:
"/// The modelled `TagKey` rules, from
/// `smithy_query(query=\"shape\", shape=\"Tags,TagKeyList\")`: a `Tags` key (and a
/// `TagKeyList` member) targets `TagKey`, which is `@length(min: 1, max: 128)` with
/// `@pattern(\"^[a-zA-Z0-9 _.:/=+@-]+$\")`.
...
pub const TAG_KEY_MIN_LEN: i64 = 1;
pub const TAG_KEY_MAX_LEN: i64 = 128;
pub const TAG_KEY_PATTERN: &str = "^[a-zA-Z0-9 _.:/=+@-]+$";"

Copy 2 — services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2407-2416:
"    // `UntagResourceInput$tagKeys` -> `TagKeyList`, element `TagKey`: @length 1..128,
    // @pattern `^[a-zA-Z0-9 _.:/=+@-]+$`.
    (
        "tagKeys",
        ElementConstraints {
            max_length: Some(128),
            min_length: Some(1),
            pattern: Some(r"^[a-zA-Z0-9 _.:/=+@-]+$"),
        },
    ),"

Only copy 1 is model-pinned: services/service-com-amazonaws-launchwizard-launchwizard/tests/all/resource_tagging.rs:78-92 (`the_tag_rules_the_handlers_enforce_are_the_models`) reads `TagKey`'s traits out of `smithy-model/` and asserts them against `state::TAG_KEY_MIN_LEN` / `TAG_KEY_MAX_LEN` / `TAG_KEY_PATTERN`. Copy 2 is exercised only through the literal expected string in services/service-com-amazonaws-launchwizard-launchwizard/tests/all/validation_messages.rs:227-229 and :337-341 (`a_tag_key_is_reported_at_its_collection…` rows), which assert the rendered message, not that the two copies agree.

## Steps to Reproduce

1. Read services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:1854-1858 — the `TagKey` length/pattern constants the handlers enforce
2. Read services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2407-2416 — the same three values repeated as literals for the `tagKeys` element constraints
3. Change one copy (e.g. the pattern) and run `cargo test -p service-com-amazonaws-launchwizard-launchwizard`: the model-pinning test only covers the constants, so the other copy can state a rule nothing enforces
