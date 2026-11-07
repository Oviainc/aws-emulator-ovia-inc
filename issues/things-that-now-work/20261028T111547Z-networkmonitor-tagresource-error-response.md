Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# TagResource's tag-ceiling ValidationException is a free-form "in this emulator" sentence, not the clause construction the crate requires for every refusal it authors

_Filed: 2026-10-28T11:15:47Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource applies the per-resource tag ceiling to the MERGED tag set and refuses a write past it with the modelled 400 `ValidationException`. The message it states is hand-written in the handler: it names the emulator ("in this emulator"), interpolates the request's `resourceArn` and a count this service computed, and is built with `format!` instead of the crate's shared clause renderer.

That contradicts the rule this crate states for every refusal its own code authors: `src/state.rs`'s module doc says such a refusal is stated in AWS's constraint-violation construction and is built through the framework's OWN clause renderer (`stated_violation` / `stated_requirement`) "so a refusal a handler raises reads exactly like the one the boundary validator renders for a modelled constraint", with exactly one exception — a refusal "whose AWS sentence is known as a whole", for which the module doc names the three cases (`TCP_PORT_REQUIRED_MESSAGE`, `aggregation_period_message`, `monitor_not_found_message`). The tag ceiling is not one of those three, and its message is plainly not an AWS sentence (no AWS service says "in this emulator").

The rule the refusal enforces is a MODELLED constraint the handler applies itself, which is precisely the case the construction rule covers: the crate's own constant doc says the ceiling's value "is the modelled `TagMap`'s own `@length(min: 0, max: 200)`, which is the most pairs any response can carry". The same handler states its other authored refusal — the reserved `aws:` tag-key rule — through the shared renderer, 40 lines earlier (`check_tag_keys` -> `stated_requirement`), so one handler answers the same caller with two different refusal shapes for two rules it applies itself. The handler's own module doc enumerates its refusals (reserved keys via `check_tag_keys`, 404 for an unknown ARN) and does not mention this one at all.

## Evidence

The rule, src/state.rs:26-38 (module doc, "How a refusal this service authors is stated"):
```
//! A rule the model states in prose but its constraints cannot express is
//! applied by the service, so the service also has to STATE the refusal. It
//! states it the way AWS states a constraint failure — the
//! `N validation error(s) detected: Value '<submitted>' at '<member path>'
//! failed to satisfy constraint: <requirement>` construction, built from the
//! request — and it builds it through the framework's OWN clause renderer
//! ([`stated_violation`], [`stated_requirement`]), so a refusal a handler raises
//! reads exactly like the one the boundary validator renders for a modelled
//! constraint. A refusal whose AWS sentence is known as a whole
//! ([`TCP_PORT_REQUIRED_MESSAGE`], [`aggregation_period_message`],
//! [`monitor_not_found_message`]) keeps that sentence instead.
```

The code that breaks it, src/ops/tag_resource.rs:229-236:
```
        if merged_len > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(validation(format!(
                "a monitor or probe keeps at most {} tags in this emulator; tagging {} with these keys would give it {}",
                crate::state::MAX_TAGS_PER_RESOURCE,
                input.resource_arn,
                merged_len
            )));
        }
```

That the ceiling is a modelled constraint the handler applies itself (so the construction rule, not the "known sentence" exception, governs it) — src/state.rs:250-255:
```
/// Its VALUE is the modelled `TagMap`'s own `@length(min: 0, max: 200)`, which
/// is the most pairs any response can carry: a resource holding more could not
/// be reported. AWS enforces a tag quota of its own, which may differ; a write
/// past this ceiling is refused with the modelled `ValidationException` naming
/// the bound, since `TagResource` models no quota error.
```
and Smithy agrees that the bound is modelled: `TagResourceInput$tags` -> `"tags": {"target": "TagMap", "required": true, "length": {"min": 0, "max": 200}}`.

The sibling refusal in the SAME handler that does go through the shared renderer, src/ops/tag_resource.rs:186-190:
```
        crate::state::check_tag_keys(
            &crate::state::member_path("tags"),
            input.tags.keys().map(String::as_str),
        )
        .map_err(validation)?;
```
which ends in src/state.rs:644-648: `Err(stated_requirement(&entry_path(map_path, key), Some(key), TAG_KEY_REQUIREMENT))`.

The test suite that states the same rule and its exceptions, tests/all/authored_refusal_messages.rs:11-32:
```
//! THE RULE: such a refusal is stated in AWS's constraint-violation
//! construction — `N validation error(s) detected: Value '<submitted>' at
//! '<member path>' failed to satisfy constraint: <requirement>` — built from the
//! request ...
//! What this suite does NOT cover is a refusal whose AWS sentence is known as a
//! whole rather than as a construction (`Destination port must be provided for
//! TCP protocol.`, `The aggregation period must be one of [30, 60].`, `<state>
//! is not a valid probe status.`)
```

Test coverage: the ceiling refusal is pinned only by status and code, never by message — tests/all/resource_tags.rs:1066-1076 asserts `status == 400` and `error_code(&refused) == "ValidationException"` and nothing about the text, so fixing the wording breaks no existing test (grep for "keeps at most" / "this emulator" across tests/ returns only the unrelated doc line resource_tags.rs:1030).

## Steps to Reproduce

1. POST /monitors with {"monitorName":"tag-at-the-bound","aggregationPeriod":60,"tags":{ ...200 distinct keys... }} -> 200.
2. POST /tags/<the monitor ARN returned above> with {"tags":{"key-200":"v"}} — one key the monitor does not already carry, so the merged set would be 201 pairs.
3. Observe HTTP 400 `ValidationException` with message "a monitor or probe keeps at most 200 tags in this emulator; tagging arn:aws:networkmonitor:...:monitor/tag-at-the-bound with these keys would give it 201".
4. Compare with the refusal the same operation states for a reserved key (POST /tags/<arn> with {"tags":{"aws:x":"v"}}), which reads "1 validation error detected: Value 'aws:x' at 'tags.aws:x' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" — the construction src/state.rs:26-38 requires of every refusal this service authors.
