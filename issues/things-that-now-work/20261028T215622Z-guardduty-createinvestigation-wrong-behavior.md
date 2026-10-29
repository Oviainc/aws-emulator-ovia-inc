Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# CreateInvestigation adds a record to the detector's own collection without moving the detector's UpdatedAt, unlike every other sub-resource create in the crate

_Filed: 2026-10-28T21:56:22Z_
_Operation: CreateInvestigation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An investigation is stored in `Detector::investigations`, one of the detector's own maps, exactly as an IPSet, a filter, a threat intel set, an entity set or a publishing destination is stored in its map. Every one of those creates states and applies the same rule — the detector now holds one more sub-resource than before, so the detector's own `UpdatedAt` moves to the instant of the create — and every matching delete applies it too. `CreateInvestigation` inserts into `detector.investigations` and leaves `detector.updated_at_millis` alone, with no comment claiming the omission is intentional.

So a detector whose only activity is initiating investigations reports a `GetDetector` `updatedAt` frozen at the instant of the last unrelated change, even though `ListInvestigations`/`GetInvestigation` show records created afterwards. The same omission was already accepted as a defect for `CreateFilter` (issue 20261025T153651Z-guardduty-createfilter-wrong-behavior--r2, fixed) and for the delete side (20261025T153651Z-guardduty-deletefilter-wrong-behavior, fixed), which is the rule the remaining six creates/deletes now all follow.

## Evidence

THE RULE, stated at the sibling create — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_ip_set.rs:252-254:
```
        detector.ip_sets.insert(id.clone(), record);
        // The detector itself changed: it holds one more sub-resource than before.
        detector.updated_at_millis = now;
```
and applied identically by create_filter.rs:323, create_threat_intel_set.rs:283, create_threat_entity_set.rs:275, create_trusted_entity_set.rs:273, create_publishing_destination.rs:269 — and by the deletes: delete_ip_set.rs:175, delete_filter.rs:169, delete_threat_intel_set.rs:202, delete_threat_entity_set.rs:187, delete_trusted_entity_set.rs:186, delete_publishing_destination.rs:182.

THE HANDLER THAT SKIPS IT — src/ops/create_investigation.rs:259-273, the whole write path, with no `detector.updated_at_millis` line and no comment about it:
```
        detector.investigations.insert(
            id.clone(),
            crate::state::InvestigationRecord::started(
                id.clone(),
                input.trigger_prompt.clone(),
                triggered_by,
                region,
                token,
                now,
            ),
        );

        Ok(CreateInvestigationOutput {
            investigation_id: id,
        })
```
(`grep` for `UpdatedAt|updated_at|detector itself` in create_investigation.rs returns no match.)

WHAT `UpdatedAt` MEANS, as this crate's own handlers quote it — src/ops/update_detector.rs:213-214: "`UpdatedAt` is \"the last-updated timestamp for the detector\" (`smithy_query(operation=\"GetDetector\")`), so it says when the detector's configuration was last written" — and src/ops/update_malware_scan_settings.rs:201-204 repeats it for a sub-record of the detector.

TEST COVERAGE: the investigation lifecycle tests exercise create → get → list; none of them asserts the detector's `updatedAt` after a create, so none pins the current behaviour.

## Steps to Reproduce

1. `POST /detector` with `{"enable": true, "features":[{"name":"AI_ANALYST","status":"ENABLED"}]}` in a Region where investigations are available.
2. `GET /detector/{detectorId}` and record `updatedAt` as T0.
3. `POST /detector/{detectorId}/investigation` with a `triggerPrompt` — the investigation is minted and inserted into `detector.investigations` (create_investigation.rs:259).
4. `GET /detector/{detectorId}` again: `updatedAt` is still T0, although the detector now holds one more record than it did.
5. Contrast with `POST /detector/{detectorId}/ipset` (CreateIPSet): a second `GetDetector` reports an `updatedAt` moved to the instant of that create (create_ip_set.rs:253-254).
