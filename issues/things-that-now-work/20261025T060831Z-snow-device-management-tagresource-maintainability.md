Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# The mutable device/task tag lookup is duplicated byte-for-byte in TagResource and UntagResource, with no counterpart to state.rs::tags_of

_Filed: 2026-10-25T06:08:31Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` already owns the resolution of a parsed `TaggedResource` to the record's tags for the READ path (`tags_of`), and `ListTagsForResource` uses it. The two WRITE paths each carry their own private copy of the same match instead: `TagResource` and `UntagResource` contain the identical nine-line `match resource.kind { Device => store.devices.get_mut(..).map(|device| &mut device.tags), Task => store.tasks.get_mut(..).map(|task| &mut task.tags) }`. One rule — "which stored map a tagged-resource kind/id resolves to" — is thus implemented three times (once in state.rs for reads, twice inline for writes), so the copies can drift: any change to how a tagged resource is resolved (for example adding the Region predicate the open TagResource/validation issue asks for) has to be made in three places and will silently apply to only some of the tagging operations if one copy is missed.

The fix is a `tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut BTreeMap<String, String>>` beside the existing `tags_of`, called from both handlers. Every response and every existing test stays byte-identical: the lookup result and the surrounding refusals are unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/tag_resource.rs:222-230`:

```
        let existing = match resource.kind {
            crate::state::TaggedResourceKind::Device => store
                .devices
                .get_mut(&resource.id)
                .map(|device| &mut device.tags),
            crate::state::TaggedResourceKind::Task => {
                store.tasks.get_mut(&resource.id).map(|task| &mut task.tags)
            }
        };
```

Copy 2 — `src/ops/untag_resource.rs:222-230`, byte-identical apart from the binding name:

```
        let tags = match resource.kind {
            crate::state::TaggedResourceKind::Device => store
                .devices
                .get_mut(&resource.id)
                .map(|device| &mut device.tags),
            crate::state::TaggedResourceKind::Task => {
                store.tasks.get_mut(&resource.id).map(|task| &mut task.tags)
            }
        };
```

The helper that already exists for the same rule — `src/state.rs:619-624`:

```
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<HashMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Device => self.devices.get(&resource.id).map(Device::tag_map),
            TaggedResourceKind::Task => self.tasks.get(&resource.id).map(Task::tag_map),
        }
    }
```

and its doc states the intent the write paths bypass (`src/state.rs:617-618`): "The lookup is by the id the ARN carries — the key each record is stored under — so a tag read and `UntagResource` address exactly the same record." `ListTagsForResource` calls it at `src/ops/list_tags_for_resource.rs:205`.

Existing coverage that must stay green and unchanged: `tests/all/resource_tagging.rs` (`tag_resource_classifies_the_resource_arn_before_it_looks_at_the_tags`, `tag_resource_enforces_the_tag_rules_on_a_resolved_resource`, `tag_resource_adds_and_replaces_tags_every_later_report_then_carries`), `tests/all/resource_tag_removal.rs`, `tests/all/resource_tags.rs` and `tests/all/create_task_tag_validation.rs::the_same_tag_map_gets_the_same_verdict_on_create_and_on_tag_resource`.

## Steps to Reproduce

1. Open `src/ops/tag_resource.rs` at lines 222-230 and `src/ops/untag_resource.rs` at lines 222-230 and diff the two blocks: they are identical.
2. Open `src/state.rs` at lines 619-624 and note `tags_of` implements the same kind/id → stored-tags rule for the read path, and that `src/ops/list_tags_for_resource.rs:205` is its only caller.
3. Observe that a change to the resolution rule (e.g. adding a Region predicate) must be edited in three places, and that nothing in the crate forces the two inline copies to stay in step.
4. Add `tags_of_mut` beside `tags_of`, call it from both handlers, and re-run `cargo test -p service-com-amazonaws-snowdevicemanagement-snowdevicemanagement` — every tagging suite must pass unchanged.
