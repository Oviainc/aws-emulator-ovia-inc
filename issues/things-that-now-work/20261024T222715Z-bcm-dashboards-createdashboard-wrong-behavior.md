Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 215626b640e395abbef6c78b28a02b35719dbae3

# CreateDashboard stores a repeated resourceTags key twice, breaking the key-merge rule TagResource implements

_Filed: 2026-10-24T22:27:15Z_
_Operation: CreateDashboard | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `215626b640e395abbef6c78b28a02b35719dbae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDashboard` persists the request's `resourceTags` list verbatim, with no merge by tag key. `TagResource`, in the same crate, states and implements the opposite rule for the same modelled `resourceTags` member (a `ResourceTagList`, which declares no `@uniqueItems`): it merges keyed by tag key so "a key the request repeats settles on its last value — the same answer as sending those entries as separate requests".

So a single create carrying the same key twice (e.g. `[{key:"env",value:"a"},{key:"env",value:"b"}]`) stores BOTH entries on the dashboard record, a state no `TagResource` call can ever produce. The stored list then violates the invariant every other tagging path assumes:

* `ListTagsForResource` projects the stored list (`list_tags_for_resource.rs:165-176`) and therefore reports two different values for one key;
* a later `TagResource` for that key finds only the FIRST match (`merged.iter_mut().find(...)`) and updates it, leaving the second, stale entry in place — so a tag update is silently only half applied and a read still reports the old value for that key;
* the per-resource tag ceiling is consumed twice by one key.

The fix is the merge the crate already implements once: resolve repeated keys at create time the way `TagResource` resolves them, so one key holds one value.

## Evidence

Create path stores the list as it arrived, with no key resolution — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/create_dashboard.rs:248-250:
```
            // Persisted at create time, as the tag list every tagging operation
            // reads and writes.
            resource_tags: input.resource_tags.unwrap_or_default(),
```
Sibling handler in THIS crate states and implements the key-merge rule — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/tag_resource.rs:209-225:
```
        // Merge, keyed by tag key: a key the resource already carries is UPDATED where it
        // already sits, ... The entries are
        // applied in request order, so a key the request repeats settles on its last
        // value — the same answer as sending those entries as separate requests.
...
        let mut merged = tags.clone();
        for tag in input.resource_tags {
            match merged.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value,
                None => merged.push(tag),
            }
        }
```
The model admits a repeated key: `ResourceTagList` is `{"type": "list", "length": {"min": 0, "max": 200}, "member": "ResourceTag"}` with no `uniqueItems` (smithy_query shape=ResourceTagList), and `ResourceTagKey`'s pattern is `^[\S\s]*$`.

Test coverage: `tests/all/tag_resource.rs:84` pins the merge rule for TagResource ("the same key twice in one request keeps the last value"), while the create-time tag table in `tests/all/resource_tags.rs:129-178` contains no row with a repeated key, so the create path's divergence is unpinned and unjustified — no comment or test in the crate states that a create may store a duplicated key.

## Steps to Reproduce

1. CreateDashboard with name="Dup Tags", widgets=[], resourceTags=[{"key":"env","value":"a"},{"key":"env","value":"b"}] — 200 with an arn.
2. ListTagsForResource with that arn — the answer reports TWO entries for key "env" (values "a" and "b"). Doing the same two entries through TagResource instead yields one entry, value "b".
3. TagResource with that arn and resourceTags=[{"key":"env","value":"c"}] — 200.
4. ListTagsForResource again — reports env=c AND env=b: the update reached only the first copy, so a read still reports the superseded value for that key.
