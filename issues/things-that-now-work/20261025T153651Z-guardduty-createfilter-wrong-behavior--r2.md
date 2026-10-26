Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# CreateFilter adds a sub-resource to the detector without moving the detector's UpdatedAt, unlike every other sub-resource create in the crate

_Filed: 2026-10-25T15:36:51Z_
_Operation: CreateFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A finding filter is a sub-resource stored on the detector record (`detector.filters`, written by `create_filter.rs:310`), but `CreateFilter` never moves `detector.updated_at_millis`. Every other implemented sub-resource create in this same crate does move it, and each one states the rule in a comment: the detector itself changed because it holds one more sub-resource than before. The result is that a caller can add a filter to a detector and `GetDetector` reports the detector as unchanged, while adding an IPSet / threat-intel set / threat-entity set / trusted-entity set / publishing destination to the same detector does move it. This is the same class of inconsistency that was already fixed for `CreateThreatIntelSet` (issue 20261024T221603Z-guardduty-createthreatintelset-wrong-behavior), so the crate's rule is settled and `CreateFilter` is the one create that still skips it.

## Evidence

The crate's stated rule, on the sibling create — services/.../src/ops/create_ip_set.rs:244-246:
```
        detector.ip_sets.insert(id.clone(), record);
        // The detector itself changed: it holds one more sub-resource than before.
        detector.updated_at_millis = now;
```
The same write in the other sibling creates: create_threat_intel_set.rs:275 `detector.updated_at_millis = now;`, create_threat_entity_set.rs:267, create_trusted_entity_set.rs:265, create_publishing_destination.rs:261.

CreateFilter, writing the same kind of sub-resource and omitting it — services/.../src/ops/create_filter.rs:309-312:
```
        let name = record.name.clone();
        detector.filters.insert(name.clone(), record);

        Ok(CreateFilterOutput { name })
```
(no `detector.updated_at_millis` assignment anywhere in the file; `grep -n "detector.updated_at" create_filter.rs` yields nothing.)

The crate's own test suite states the rule as holding over the whole class — services/.../tests/all/detector_sub_resource_creates.rs:10-15:
```
//! The rule under test, asserted over the SET of sub-resource families a detector
//! holds rather than over one of them: a call that changes the set of sub-resources a
//! detector holds changes the detector, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that change
```
and line 46-47 claims the rows are "Every sub-resource family whose create is implemented" — yet `families()` (lines 53-149) lists ipset, threatintelset, threatentityset, trustedentityset and publishingDestination only, never the filter family, so no existing test pins the current CreateFilter behaviour.

## Steps to Reproduce

1. CreateDetector with {"enable": true}; GET /detector/{detectorId} and note `updatedAt`.
2. POST /detector/{detectorId}/filter with a valid name and findingCriteria (200).
3. GET /detector/{detectorId} again: `updatedAt` is unchanged, although the detector now holds one more sub-resource.
4. For contrast, POST /detector/{detectorId}/ipset with a valid create and GET the detector again: `updatedAt` has moved (create_ip_set.rs:246).
