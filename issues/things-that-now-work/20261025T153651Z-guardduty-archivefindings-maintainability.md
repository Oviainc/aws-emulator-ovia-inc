Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# The archive/unarchive refusal-and-write rule is implemented twice, once per handler, differing only in a bool constant

_Filed: 2026-10-25T15:36:51Z_
_Operation: ArchiveFindings | Class: maintainability_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ArchiveFindings` and `UnarchiveFindings` implement the identical rule — resolve the detector for the request's Region, refuse the whole batch if any `FindingIds` entry is not held by that detector, then write the archive flag through `set_findings_archived_in` and re-refuse if the detector vanished — as two byte-for-byte equal handler bodies whose only difference is the `TARGET_ARCHIVED` constant. The shared part is logic, not boilerplate: the batch-refusal ordering ("a batch naming an id the detector does not hold archives nothing at all") and the classification of the two failure modes are duplicated, so a later change to either can be applied to one direction and not the other. The write itself already lives in one state helper; the refusal walk around it does not. Folding the resolve-plus-refuse step into that same state helper (parameterised by the target flag, as the write already is) leaves every response and every existing test unchanged.

## Evidence

Copy 1 — services/.../src/ops/archive_findings.rs:154-185:
```
        let rejection = {
            let Some(detector) = state.detector_in(&region, &input.detector_id) else {
                return Err(crate::scaffolded::Scaffolded::invalid_input(
                    crate::state::detector_id_rejection(&input.detector_id),
                ));
            };
            detector
                .first_unheld_finding_id(&input.finding_ids)
                .map(crate::state::finding_id_rejection)
        };
        if let Some(message) = rejection {
            return Err(crate::scaffolded::Scaffolded::invalid_input(message));
        }
        if state
            .set_findings_archived_in(&region, &input.detector_id, &input.finding_ids, TARGET_ARCHIVED)
            .is_none()
        { ... }
```
Copy 2 — services/.../src/ops/unarchive_findings.rs:151-183: the same block verbatim, with `const TARGET_ARCHIVED: bool = false;` (unarchive_findings.rs:193) instead of `const TARGET_ARCHIVED: bool = true;` (archive_findings.rs:195). Unarchive's own comment admits the duplication: "the same rule `ArchiveFindings` applies to the other direction of this write" (unarchive_findings.rs:159-160).

Covering tests that must stay green and unchanged: services/.../tests/all/findings_archive_state.rs (rules 1-3, including lines 296-356 on `updatedAt` moving only on a real change) and tests/all/findings_feedback.rs:18.

## Steps to Reproduce

1. Read services/.../src/ops/archive_findings.rs:154-188 and services/.../src/ops/unarchive_findings.rs:151-186 side by side: the two handler bodies are identical apart from the `TARGET_ARCHIVED` constant.
2. Note that `crate::state::set_findings_archived_in` (state.rs:418) already takes the flag as a parameter, so only the surrounding resolve-and-refuse walk is duplicated.
3. Change the refusal rule in one file (for example to tolerate an unheld finding id) and observe that nothing forces the other direction to follow.
