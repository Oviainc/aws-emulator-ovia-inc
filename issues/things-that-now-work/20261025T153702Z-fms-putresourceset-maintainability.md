Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# The ARN-keyed tag index is maintained by five hand-copied writes and four hand-copied removals instead of the one helper its own doc names

_Filed: 2026-10-25T15:37:02Z_
_Operation: PutResourceSet | Class: maintainability_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tags_by_arn` is documented as an index maintained from the authoritative record through a single writer (`set_tags`). In practice the insert-the-record's-tags-under-its-ARN step is copied inline into all four resource-creating handlers, and the removal step is copied into `delete_resource_set`'s handler while the other three deleters perform it inside state.rs. That is one rule — "the index entry equals the record's tags, keyed by the record's own ARN" — written out nine times, so a fifth taggable resource kind (or an edit to one copy) silently leaves the index disagreeing with the records the tests assert it mirrors. Routing the four Put handlers and DeleteResourceSet through one state-module helper leaves every response byte-identical and every existing test unchanged.

## Evidence

The single-writer rule the state module states — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:952-961:
```
    /// Which copy wins: the TAGS ON THE RECORD of the resource are authoritative
    /// ... and this map is maintained from them — each writer inserts the
    /// record's tags under the resource's ARN and each deleter removes the entry.
    /// A tag write therefore goes through
    /// [`ComAmazonawsFmsAwsfms20180101State::set_tags`], which writes the record
    /// and refreshes this entry from the SAME value, so the two cannot disagree;
```
and the helper that implements it — state.rs:1186-1213 (`pub fn set_tags(... ) -> bool { ... self.tags_by_arn.insert(arn, tags); ... }`).

The four hand-copied inserts, none of which calls it:
- src/ops/put_resource_set.rs:314-316 `store.tags_by_arn.insert(record.arn.clone(), record.tags.clone());`
- src/ops/put_policy.rs:592-594 `store.tags_by_arn.insert(record.policy_arn.clone(), record.tags.clone());`
- src/ops/put_apps_list.rs:315-317 `store.tags_by_arn.insert(record.list_arn.clone(), record.tags.clone());`
- src/ops/put_protocols_list.rs:321-323 `store.tags_by_arn.insert(record.list_arn.clone(), record.tags.clone());`

The removal, inline in one handler but inside state.rs for the other three kinds:
- src/ops/delete_resource_set.rs:186-188 `store.tags_by_arn.remove(&removed.arn);`
- vs state.rs:1583 (`delete_customer_apps_list`), state.rs:1598 (`delete_customer_protocols_list`), state.rs:1632 (`delete_policy`).

Tests that pin the index and must keep passing unchanged: tests/all/resource_set_deletion.rs:197-235 and :323, tests/all/tag_removal.rs:311, tests/all/tagging_across_resource_kinds.rs:374, tests/all/apps_list_lifecycle.rs:562 and :674, tests/all/protocols_list_lifecycle.rs:553, tests/all/policy_identifier_lookup.rs:300, tests/all/policy_administrator_gate.rs:227 and :269.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:952-961 and read the stated rule that a tag write goes through `set_tags`
2. grep -n tags_by_arn services/service-com-amazonaws-fms-awsfms-20180101/src — five writes (four in ops/put_*.rs, one in state.rs::set_tags) and four removals (one in ops/delete_resource_set.rs, three in state.rs)
3. Note that each of the four Put copies differs only in the record field holding the ARN, so the index-maintenance rule exists in nine places that can drift independently
