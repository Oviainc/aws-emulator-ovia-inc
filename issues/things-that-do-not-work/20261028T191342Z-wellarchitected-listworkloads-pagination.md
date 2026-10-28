Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6715c947a740417962c3143a65f87ddf3b03a07e

# ListWorkloads pages on the mutable WorkloadName, so a rename between two pages serves a workload twice or never serves it at all

_Filed: 2026-10-28T19:13:42Z_
_Operation: ListWorkloads | Class: pagination_

> _Note: this issue was identified against the code base at commit `6715c947a740417962c3143a65f87ddf3b03a07e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListWorkloads orders its enumeration by, and encodes its `NextToken` cursor from, the pair `(WorkloadName, WorkloadId)`. `WorkloadName` is a member `UpdateWorkload` writes, so the key a page cursor resumes from can MOVE while a client is walking the pages. The resume is computed with `order.partition_point(|candidate| candidate <= key)` over a freshly recomputed order, so:

* renaming an already-served workload to a name that sorts AFTER the cursor pushes it past the resume point, and the next page serves it a SECOND time;
* renaming a not-yet-served workload to a name that sorts BEFORE the cursor pushes it behind the resume point, and it is NEVER served, even though it existed for the whole walk.

This crate states the rule this handler breaks, in its own words, in four sibling paged readers, each of which deliberately keys its order and its cursor on something immutable (ARN, or the workload id) for exactly this reason. `ListWorkloads` is the only paged reader in the crate whose cursor half is a caller-writable member. The crate's own ListWorkloads paging test asserts the property that is lost ("paging returned a workload twice"), but only over a walk with no rename in it.

## Evidence

Side 1 — the order and the cursor are keyed on the NAME.

`src/state.rs:7195-7205`:
```
    /// The service's stable list order: workload name, then id to break ties between
    /// two names that normalize apart but sort equal.
    pub fn workload_order(&self) -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = self
            .workloads
            .values()
            .map(|w| (w.workload_name.clone(), w.workload_id.clone()))
            .collect();
        keys.sort();
        keys
    }
```

`src/ops/list_workloads.rs:178-202`:
```
        let order = store.workload_order();
        // The cursor is start-EXCLUSIVE: the page resumes just past the last key already
        // returned, so every workload is enumerated exactly once across the pages.
        let start = match &cursor {
            Some(key) => order.partition_point(|candidate| candidate <= key),
            None => 0,
        };
...
            Some((name, id)) if matching.len() > page.len() => {
                Some(crate::state::encode_next_token(name, id))
            }
```

`src/state.rs:8021-8033` — the token really carries the name:
```
/// Encode a `ListWorkloads` cursor: the last `(workload name, workload id)` served.
pub fn encode_next_token(workload_name: &str, workload_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "n": workload_name, "i": workload_id }))
}
```

Side 2 — the name is writable at any time. `src/ops/update_workload.rs:337-339`:
```
        if let Some(workload_name) = input.workload_name {
            record.workload_name = workload_name;
        }
```

Side 3 — the crate states that `WorkloadName` must NOT be an ordering/cursor key, naming `UpdateWorkload` as the reason, in two sibling paged readers:

`src/ops/list_notifications.rs:288-294`:
```
        // The model publishes no order for `NotificationSummaries`, so this is the
        // emulator's own: the pair that names the review — the key of the resource holding
        // it beside the lens alias the review is one review under — whose two halves are
        // each fixed for the life of what they name, where `WorkloadName` is a member
        // `UpdateWorkload` can move and `TemplateName` one `UpdateReviewTemplate` can, so
        // the order is total and stable across requests.
```

`src/ops/list_profile_notifications.rs:228-233`:
```
        // The model publishes no order for `ProfileNotificationSummaries`, so this is the
        // emulator's own: the pair that names the association, whose two halves are each
        // fixed for the life of the resource they name — where `WorkloadName` is a member
        // `UpdateWorkload` can move and `ProfileName` one `UpdateProfile` can — so the
        // order is total and stable across requests.
```

and the two sibling resource listings spell out the property an immutable key buys, which ListWorkloads therefore does not have:

`src/ops/list_profiles.rs:182-184`: "The cursor is start-EXCLUSIVE: the page resumes just past the ARN already served, so every profile is enumerated exactly once across the pages, and a profile created or deleted between two pages moves nothing already served."

`src/ops/list_review_templates.rs:178-180`: "The cursor is start-EXCLUSIVE: the page resumes just past the ARN already served, so every template is enumerated exactly once across the pages, and a template created or deleted between two pages moves nothing already served."

Test coverage — the exactly-once property is asserted, but only for a walk with no rename in it, so nothing pins the broken behaviour: `tests/all/workload_lifecycle.rs:452-457`:
```
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: paging returned a workload twice: {seen:?}"
            );
```
That suite collects `WorkloadId`s into a set and compares sets (`tests/all/workload_lifecycle.rs:401-459`); it does not assert the page ORDER, and it performs no `UpdateWorkload` between pages. `tests/all/workload_lifecycle.rs:466` (`WorkloadNamePrefix` filters the summaries, and the filter composes with paging) likewise walks without renaming.

## Steps to Reproduce

1. POST /workloads three times to create workloads named `aaa`, `bbb` and `ccc` (each with a distinct ClientRequestToken)
2. POST /workloadsSummaries with {"MaxResults": 1} — the page carries the `aaa` summary and a NextToken whose cursor is ("aaa", <id of aaa>) (src/state.rs:8022)
3. Duplicate case: PATCH /workloads/<id of aaa> with {"WorkloadName": "zzz"} (src/ops/update_workload.rs:337-339)
4. POST /workloadsSummaries with {"MaxResults": 1, "NextToken": <token from step 2>} and keep following NextToken: the recomputed order is bbb, ccc, zzz(=aaa) and partition_point over ("aaa", id) now returns 0, so the walk serves `bbb`, `ccc` and then the already-served workload a SECOND time — the condition tests/all/workload_lifecycle.rs:452-457 forbids
5. Skip case: restart from step 2 and instead PATCH /workloads/<id of ccc> with {"WorkloadName": "aa0"}
6. Follow the NextToken from step 2 again: the order is aa0(=ccc), aaa, bbb and partition_point over ("aaa", id of aaa) returns 2, so the remaining pages serve only `bbb` and the renamed workload is never enumerated at all, although it existed for the whole walk
7. Compare with src/ops/list_review_templates.rs:178-184 and src/ops/list_profiles.rs:182-188, which resume from an immutable ARN and are immune to both cases
