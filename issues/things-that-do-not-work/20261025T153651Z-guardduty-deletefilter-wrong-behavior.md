Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# DeleteFilter's comment claims a sub-resource delete is not a change to the detector, contradicting every sibling delete in the same crate

_Filed: 2026-10-25T15:36:51Z_
_Operation: DeleteFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteFilter` removes a filter from `detector.filters` and deliberately leaves the detector's own `UpdatedAt` where it was, justifying it in a comment with the rule "a filter is its own resource, and deleting one is not a change to the detector's configuration". Every other sub-resource delete in this crate asserts and implements the opposite rule: the first accepted delete means the detector holds one fewer LIVE sub-resource, so the detector's `UpdatedAt` moves. The crate therefore holds two contradictory rules for the same question, and which one a caller observes depends only on which collection the sub-resource lives in.

## Evidence

DeleteFilter's rule — services/.../src/ops/delete_filter.rs:156-166:
```
        if detector.filters.remove(&input.filter_name).is_none() {
            return Err(bad_request(crate::state::filter_name_rejection(
                &input.filter_name,
            )));
        }

        // `DeleteFilterResponse` declares no members
        // (`smithy_query(operation="DeleteFilter")`). The detector's own timestamps do
        // not move: a filter is its own resource, and deleting one is not a change to
        // the detector's configuration.
```
The contrary rule, in the sibling delete — services/.../src/ops/delete_ip_set.rs:171-176:
```
        if accepted {
            // The detector holds one fewer LIVE sub-resource than before, which is
            // the same change `CreateIPSet` records on it in the other direction. A
            // repeated delete changed nothing, so it moves nothing here either.
            detector.updated_at_millis = now;
        }
```
The same write in delete_threat_intel_set.rs:202, delete_threat_entity_set.rs:187 and delete_trusted_entity_set.rs:186, and the rule restated in state.rs:2845-2851:
```
    /// `None` when this detector holds no such set at all; `Some(true)` when this
    /// call is the one that accepted the deletion ... because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
```
No test pins DeleteFilter's effect on the detector: tests/all/filter_delete.rs and filter_lifecycle.rs never read `GetDetector` (`grep -n "GetDetector|get-detector" tests/all/filter_lifecycle.rs` -> no matches), while tests/all/trusted_entity_set_delete.rs:358-365 asserts "the first delete changed the detector, so its updatedAt must move".

## Steps to Reproduce

1. CreateDetector with {"enable": true} and create a filter under it.
2. GET /detector/{detectorId} and note `updatedAt`.
3. DELETE /detector/{detectorId}/filter/{filterName} (200).
4. GET /detector/{detectorId}: `updatedAt` is unchanged, although the detector holds one fewer sub-resource.
5. Repeat with an IPSet instead of a filter: the detector's `updatedAt` moves on the first delete (delete_ip_set.rs:175).
