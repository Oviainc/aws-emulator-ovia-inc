Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# GetReport gates every report on a term grant, ignoring a report's PASSTHROUGH acceptanceType

_Filed: 2026-10-26T08:46:19Z_
_Operation: GetReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetReport` refuses the download of ANY report whose `termToken` is not a grant this service minted for that report's term document. The decision consults only the term digest; the report's `acceptanceType` — which the catalog stores, projects into `ReportSummary`/`ReportDetail`, and even prints into the seeded term document — is never read by any decision in the crate.

The model distinguishes the two acceptance types by exactly this requirement: `AcceptanceType$PASSTHROUGH` is documented as "Do not require explicit click-through acceptance of the Term associated with this Report", while `AcceptanceType$EXPLICIT` is "Require explicit click-through acceptance of the Term associated with this Report." A term grant in this crate IS that explicit click-through acceptance: it is minted only by `GetTermForReport` ("MINTS the `termToken` … the acceptance receipt `GetReport` requires"). So for the catalog's PASSTHROUGH reports the handler enforces precisely the acceptance the model says those reports do not require, and a caller that has not called `GetTermForReport` gets a 400 `ValidationException` for a report the model marks as needing no acceptance.

The same rule is restated in this crate's own state module beside the stored field ("`EXPLICIT` requires click-through acceptance of the report's term, `PASSTHROUGH` does not"), and no comment, constant or test in the crate gives a reason for consulting it nowhere. The seeded catalog contains both kinds (e.g. state.rs:2635 `acceptance_type: "PASSTHROUGH"`, state.rs:2705 `acceptance_type: "EXPLICIT"`), so the collapsed rule is observable over HTTP on the emulator's own data.

## Evidence

Smithy model (`smithy_query` shape=AcceptanceType), member docs state the behaviour themselves:
  PASSTHROUGH: "Do not require explicit click-through acceptance\nof the Term associated with this Report"
  EXPLICIT: "Require explicit click-through acceptance of the\nTerm associated with this Report."

Handler, which applies the acceptance gate unconditionally — services/service-com-amazonaws-artifact-artifact/src/ops/get_report.rs:203-221:
```
        if !store.grant_covers(&input.term_token, report) {
            ...
            return Err(GetReportErrorModeled::ValidationException(
                crate::state::invalid_token(),
            )
            .into());
```
The check it calls looks at the term digest only — src/state.rs:1548-1552:
```
    pub fn grant_covers(&self, token: &str, report: &Report) -> bool {
        self.term_tokens
            .get(token)
            .is_some_and(|grant| grant.term_digest == report.term.digest)
    }
```
The grant is the explicit acceptance receipt — src/ops/get_term_for_report.rs:4-8: "Serves the term document attached to one report version, and MINTS the `termToken` … the acceptance receipt `GetReport` requires."

The crate stores and restates the rule but consults it nowhere — src/state.rs:514-516:
```
    /// One of `AcceptanceType`'s values: `EXPLICIT` requires click-through acceptance
    /// of the report's term, `PASSTHROUGH` does not.
    pub acceptance_type: String,
```
Every use of `acceptance_type` in src/ is a projection or a seed: state.rs:574 and state.rs:604 (`to_summary` / `to_detail`), state.rs:3061 (seed), state.rs:3105-3114 (term document text). No branch reads it.

Test coverage: tests/all/report_workflow.rs:541-639 `a_report_downloads_only_under_a_term_grant_issued_for_it` pins the refusal for the whole catalog, including the PASSTHROUGH entries (harness.rs:94, 102, 150-182, 214), with the stated reason "A token this service never issued is not a token: no URL, a modelled 400" — a reason the model sentence above contradicts for PASSTHROUGH reports, so this fixture encodes the gap rather than justifying it. tests/all/report_workflow.rs:178-181 shows `acceptanceType` is asserted only as a reported value.

## Steps to Reproduce

1. Start the emulator and call GET /v1/report/list to find a report whose reported `acceptanceType` is `PASSTHROUGH` (e.g. the first seeded catalog entry, src/state.rs:2635).
2. Without calling GetTermForReport, call GET /v1/report/get?reportId=<that id>&termToken=anything (a value satisfying the modelled `^[a-zA-Z0-9_\-\s]*$`, 1..256 pattern).
3. Observe HTTP 400 `ValidationException` with the fixed invalid-token message and no `documentPresignedUrl`: the handler required the explicit click-through acceptance that `AcceptanceType$PASSTHROUGH` documents as not required.
4. Call GET /v1/report/getTermForReport?reportId=<same id>, then repeat step 2 with the minted token: it now returns 200 — showing the download is gated on an acceptance regardless of the report's acceptanceType.
5. Repeat steps 2-4 with a report whose `acceptanceType` is `EXPLICIT` (src/state.rs:2705) and observe the handler behaves identically, i.e. the stored attribute changes nothing.
