Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d2641f8668c2a4abe0c9d800c33fcdc9b555a45a

# CreateThreatIntelSet leaves the detector's UpdatedAt untouched, contradicting DeleteThreatIntelSet's own comment and every sibling create

_Filed: 2026-10-24T22:16:03Z_
_Operation: CreateThreatIntelSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d2641f8668c2a4abe0c9d800c33fcdc9b555a45a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Adding a sub-resource to a detector moves the detector's `UpdatedAt` everywhere else in this crate: `CreateIPSet`, `CreateThreatEntitySet`, `CreateTrustedEntitySet` and `CreatePublishingDestination` all set `detector.updated_at_millis = now` after inserting, and `DeleteThreatIntelSet` sets it when a delete is accepted — justifying itself with the claim that this is "the same change `CreateThreatIntelSet` records on it in the other direction". `CreateThreatIntelSet` records no such change: it inserts the record and returns without touching `detector.updated_at_millis`. So the stated invariant is false for exactly one operation, and `GetDetector`'s `UpdatedAt` moves when a threat list is deleted but not when one is created — the two halves of one lifecycle disagree.

## Evidence

The claim, services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_threat_intel_set.rs:197-203:
```
        if accepted {
            // The detector holds one fewer LIVE sub-resource than before, which is
            // the same change `CreateThreatIntelSet` records on it in the other
            // direction. A repeated delete changed nothing, so it moves nothing here
            // either.
            detector.updated_at_millis = now;
        }
```
The create it names does not record it — src/ops/create_threat_intel_set.rs:270-281 (the end of the handler):
```
        detector.evict_deleted_threat_intel_sets(None);
        detector.threat_intel_sets.insert(id.clone(), record);

        Ok(CreateThreatIntelSetOutput {
            threat_intel_set_id: id,
        })
```
The sibling creates do — src/ops/create_ip_set.rs:232-234:
```
        detector.ip_sets.insert(id.clone(), record);
        // The detector itself changed: it holds one more sub-resource than before.
        detector.updated_at_millis = now;
```
and src/ops/create_threat_entity_set.rs:259-263:
```
        detector.threat_entity_sets.insert(id.clone(), record);
        // The detector itself changed: it holds one more sub-resource than before,
        // the same rule `CreateTrustedEntitySet` and `CreateIPSet` follow for their
        // own sub-resources.
        detector.updated_at_millis = now;
```
No test pins the current reading: a grep of tests/ for the detector's `updatedAt` finds assertions for the IPSet delete (tests/all/ip_set_update_and_delete.rs:588), the trusted-entity-set delete (tests/all/trusted_entity_set_delete.rs:332) and the scan settings (tests/all/malware_scan_settings.rs:454) only.

## Steps to Reproduce

1. CreateDetector to get a detectorId; GET /detector/{detectorId} and note updatedAt
2. POST /detector/{detectorId}/threatintelset with a valid name/format/location/activate
3. GET /detector/{detectorId}: updatedAt is unchanged
4. For contrast, POST /detector/{detectorId}/ipset and GET /detector/{detectorId}: updatedAt has moved
5. DELETE /detector/{detectorId}/threatintelset/{id} and GET /detector/{detectorId}: updatedAt moves, so the create and the delete of one resource family disagree
