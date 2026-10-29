Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2bc7d6b091147721cb0b696f6323d28be421a007

# A filter's ARCHIVE action is stored and reported but never applied to any finding, so a suppression filter suppresses nothing

_Filed: 2026-10-29T04:56:58Z_
_Operation: CreateFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2bc7d6b091147721cb0b696f6323d28be421a007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateFilter`/`UpdateFilter` accept `Action` (the `FilterAction` enum, `NOOP | ARCHIVE`), store it on the filter record and report it back from `GetFilter`. Nothing in the crate ever ACTS on it. A filter saved with `action: "ARCHIVE"` and criteria that match stored findings leaves every one of those findings with `service.archived == false`: `ListFindings`, `GetFindings` and `GetFindingsStatistics` keep reporting them exactly as they would with `action: "NOOP"`, and `CreateSampleFindings` writes every generated finding with `archived: false` without ever consulting the detector's filters.

So the two members of the modelled enum are observationally identical: the only effect of `ARCHIVE` is the string a later `GetFilter` echoes. The action is a hollow round-trip rather than a rule.

This is not behaviour the crate never attempts — both halves of applying it already exist and are already called from elsewhere in this same crate: `crate::state::finding_matches(finding, criteria)` (state.rs:5310), which `ListFindings` (list_findings.rs:189) and `GetFindingsStatistics` (get_findings_statistics.rs:213) use to decide whether a finding satisfies a `FindingCriteria`, and `Detector::set_findings_archived` (state.rs:2754), the one archive write both `ArchiveFindings` and `UnarchiveFindings` go through. The filter's own criteria are even validated against the finding-field catalogue on the way in (create_filter.rs:208, update_filter.rs:175) so that "a saved filter carries only criteria a findings read accepts" — the crate goes to the trouble of guaranteeing the criteria are evaluable, and then never evaluates them.

## Evidence

WHAT THE MODEL SAYS THE MEMBER DOES — `smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`:
```
<p>Specifies the action that is to be applied to the findings that match the filter.</p> <p>Default: NOOP</p>
```
and the value set it can carry, `smithy_query(query="shape",shape="FilterAction")`:
```
"FilterAction": {"type": "enum", "values": ["NOOP", "ARCHIVE"]}
```

WHAT THE HANDLER DOES WITH IT — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_filter.rs:278-285 (stored, nothing else):
```
            // "Default: NOOP" when the request names no action
            // (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`).
            action: Some(
                input
                    .action
                    .clone()
                    .unwrap_or_else(|| crate::state::FILTER_ACTION_NOOP.to_string()),
            ),
```
src/ops/update_filter.rs:226-228 (overwritten, nothing else):
```
        if let Some(action) = input.action.as_ref() {
            filter.action = Some(action.clone());
        }
```
src/ops/get_filter.rs:185-187 — the only other reader, which merely echoes it:
```
                .action
                ...
                .unwrap_or_else(|| crate::state::FILTER_ACTION_NOOP.to_string()),
```

NOBODY ELSE READS IT. A grep of the whole crate for a filter's action finds exactly those three sites:
```
$ grep -rn 'FILTER_ACTION|filter\.action|\.action\b' services/.../src
state.rs:2122: pub const FILTER_ACTION_NOOP: &str = "NOOP";
state.rs:4147:     /// `FilterAction`: `ARCHIVE` or [`FILTER_ACTION_NOOP`].
ops/update_filter.rs:226-227 ; ops/get_filter.rs:185-187 ; ops/create_filter.rs:282-284
```
and a grep for the stored filter map shows it is only ever touched by the five filter CRUD handlers — never by a findings path: `create_filter.rs:245,260,261,318`, `update_filter.rs:195,208`, `delete_filter.rs:159`, `get_filter.rs:169`.

THE TWO HELPERS THE CRATE ALREADY HAS AND DOES NOT CALL HERE — src/state.rs:5310-5323:
```
pub fn finding_matches(
    finding: &FindingRecord,
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<bool, CriterionRejection> {
```
(called from src/ops/list_findings.rs:189 and src/ops/get_findings_statistics.rs:213) and src/state.rs:2742-2754:
```
    /// Archiving and unarchiving are the two directions of ONE write over the
    /// `FindingIds` a request names ... so both go through here and the LAST call to land
    /// decides the state.
    pub fn set_findings_archived(&mut self, finding_ids: &[String], archived: bool, now: i64) {
```

AND THE CRATE GUARANTEES THE CRITERIA ARE EVALUABLE BEFORE SAVING THEM — src/ops/create_filter.rs:289-293:
```
            // Stored as sent, once every criterion key has been resolved as a field
            // of a finding above: a saved filter carries only criteria a findings
            // read accepts, so `GetFilter` reports back exactly what the create
            // accepted.
```

WHERE A GENERATED FINDING IS WRITTEN WITHOUT CONSULTING ANY FILTER — src/ops/create_sample_findings.rs:244-245:
```
                archived: false,
                sample: true,
```

TEST COVERAGE — nothing pins the current behaviour. tests/all/filter_lifecycle.rs:211-221 creates one filter per `FilterAction` member and only asserts the value round-trips ("`Action` is the whole `FilterAction` value set plus \"named nothing\""); tests/all/finding_field_catalog.rs:247-270 creates `"action": "ARCHIVE"` filters whose criterion is `{"equals": ["a-value-no-finding-carries"]}` — deliberately matching no finding — and asserts only that the criterion round-trips; tests/all/findings_archive_state.rs covers only the explicit `ArchiveFindings`/`UnarchiveFindings` write. No test asserts that an ARCHIVE filter leaves a matching finding unarchived.

## Steps to Reproduce

1. Create a detector: `POST /detector` with `{"enable": true}`; note the `detectorId`.
2. Generate findings: `POST /detector/{detectorId}/findings/create` (`CreateSampleFindings`) with `{"findingTypes": ["Recon:EC2/Portscan"]}`. The finding is written with `archived: false` (create_sample_findings.rs:244).
3. Save a suppression filter over exactly that finding: `POST /detector/{detectorId}/filter` with `{"name":"suppress-portscan","action":"ARCHIVE","findingCriteria":{"criterion":{"type":{"equals":["Recon:EC2/Portscan"]}}}}`. It answers 200 and `GET /detector/{detectorId}/filter/suppress-portscan` reports `"action": "ARCHIVE"`.
4. Read the finding back: `POST /detector/{detectorId}/findings` (`GetFindings`) with the finding id. Expected per the member's own documentation ("the action that is to be applied to the findings that match the filter"): the matching finding is archived. Actual: `service.archived` is `false` and the finding is still listed by `POST /detector/{detectorId}/findings?...` (`ListFindings`) with no criteria — the saved action changed nothing.
5. Repeat step 3 with `"action":"NOOP"` instead: every observable response in steps 3-4 is byte-identical, so the two members of the modelled enum cannot be told apart.
6. Contrast with the explicit write the crate does implement: `POST /detector/{detectorId}/findings/archive` (`ArchiveFindings`) with the same finding id flips `service.archived` to `true` through `Detector::set_findings_archived` (state.rs:2754) — the helper the filter path never calls.
