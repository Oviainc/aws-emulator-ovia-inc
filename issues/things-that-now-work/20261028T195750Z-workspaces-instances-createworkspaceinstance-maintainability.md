Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b308822e8dcfad577e47bde6f349d7c8c8334529

# Collapse the duplicated tag-capacity refusal into one shared helper

_Filed: 2026-10-28T19:57:49Z_
_Operation: CreateWorkspaceInstance | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b308822e8dcfad577e47bde6f349d7c8c8334529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a request must not leave a WorkSpace Instance holding more than MAX_TAGS_PER_INSTANCE tags" is implemented TWICE in this crate's own handler code — once in `CreateWorkspaceInstance` over the merged `TagSpecifications`+`Tags` set, and once in `TagResource` over the merged stored+request set. Both copies compute the same comparison against `crate::state::MAX_TAGS_PER_INSTANCE`, build the same `why` sentence with the same format string, and raise the same `ValidationException` through `crate::state::field_validation_failed`; only the member name passed to `field_validation_failed` differs ("TagSpecifications" vs "Tags").

This is the ONE cross-field rule of this crate that is not centralized. Every sibling rule lives in exactly one place in `state.rs` and says so in its own doc: the reserved-tag-key rule, the Availability-Zone rule, and the per-volume-type configuration rule are each applied through a single shared function precisely so the call sites cannot drift. The tag-capacity rule has no such function, so the two copies can drift on the bound they compare against, on the wording, or on which side of `>` the bound sits — and a third caller that needs it (another operation that grows a stored tag set) has no helper to reach for.

The fix is a `state::require_tag_capacity(member, &tags) -> Result<(), ValidationException>` next to `reject_reserved_tag_keys`, called from both handlers with their own member name. Every response byte and every existing test stays unchanged: the member names, the message text and the error shape are exactly the ones the two sites already produce.

## Evidence

Copy 1 — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_workspace_instance.rs:358-367:
```
        if tags.len() > crate::state::MAX_TAGS_PER_INSTANCE {
            let why = format!(
                "the request would leave the WorkSpace Instance holding more than the {} tags a resource can hold",
                crate::state::MAX_TAGS_PER_INSTANCE
            );
            return Err(CreateWorkspaceInstanceErrorModeled::ValidationException(
                crate::state::field_validation_failed("TagSpecifications", why),
            )
            .into());
        }
```

Copy 2 — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/tag_resource.rs:181-194:
```
        if merged.len() > crate::state::MAX_TAGS_PER_INSTANCE {
            // Names the member and the constraint it violated, and NONE of what the
            // caller sent: ...
            let why = format!(
                "the request would leave the WorkSpace Instance holding more than the {} tags a resource can hold",
                crate::state::MAX_TAGS_PER_INSTANCE
            );
            return Err(TagResourceErrorModeled::ValidationException(
                crate::state::field_validation_failed("Tags", why),
            )
            .into());
        }
```

The crate's own stated convention for exactly this kind of rule — services/.../src/state.rs:2438-2444 (`reject_reserved_tag_keys`): "Shared by every operation that stores caller tags, so one reserved key is refused the same way whichever member carried it"; state.rs:1301-1303 (`require_zone_in_region`): "The ONE place that rule is applied, so both sides of it cannot drift apart"; state.rs:1381-1383 (`require_accepted_configuration`): "The ONE place that rule is applied, so the create of a volume and the modification of one cannot drift apart". `CreateWorkspaceInstance`'s own comment at create_workspace_instance.rs:353-355 even points at the other copy — "exactly as `TagResource` refuses a merge that would grow the set past the bound" — i.e. the handler knows it is restating a rule another handler already implements.

Existing tests that pin both copies (and that the refactor must leave untouched):
- tests/all/create_tag_specifications.rs:359 `the_tag_set_a_specification_builds_cannot_exceed_what_the_model_reports`, which also asserts `FieldList` names `TagSpecifications` (tests/all/create_tag_specifications.rs:343-346) — the create-side copy.
- tests/all/workspace_instance_lifecycle.rs:731 `the_stored_tag_set_cannot_grow_past_what_the_model_can_report` — the `TagResource` copy.
- tests/all/tag_list_merge_rule.rs:265-269 treats it as one rule: "A row at the modelled maximum plus the base set would pass the bound the resource's reported `TagList` can carry, which is its own refusal rule."

## Steps to Reproduce

1. Open services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_workspace_instance.rs and read lines 358-367.
2. Open services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/tag_resource.rs and read lines 181-194.
3. Observe the same bound comparison, the same format string and the same `field_validation_failed` construction in both, differing only in the member name argument.
4. Compare with state.rs:2445 `reject_reserved_tag_keys`, state.rs:1306 `require_zone_in_region` and state.rs:1393 `require_accepted_configuration`, each documented as the ONE place its rule is applied and each called from both the create and the mutating path.
5. Confirm no `state.rs` helper exists for the tag-capacity rule: grep the crate for `MAX_TAGS_PER_INSTANCE` and see that the only comparisons against it are the two handler copies above.
