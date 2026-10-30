Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ede2de3f7102f867619bb62880127888c08c3e6a

# GetReportMetadata reports a different termArn per report version, although the crate holds one term per report

_Filed: 2026-10-29T08:35:56Z_
_Operation: GetReportMetadata | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ede2de3f7102f867619bb62880127888c08c3e6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ReportDetail$termArn` is built from the report id AND the resolved version, so two versions of one report are reported under two different term ARNs — i.e. the response names two distinct term resources. The crate's own state model says the opposite: a report has exactly ONE term document, it governs every version, and that is why a term grant minted through one version authorises the others. Everything else in the crate follows the one-term-per-report rule: the term document is a field of `Report` (not of `ReportVersion`), the data plane serves the same `report.term` bytes under every version, and `grant_covers` keys authorisation on the term document's content digest alone, ignoring the version the acceptance was requested through. Only the projected `termArn` disagrees, so a client that keys a term acceptance off `termArn` is told it must accept two terms for a report whose single term this service will authorise with one token.

## Evidence

The crate's rule, on the field that holds the term — services/service-com-amazonaws-artifact-artifact/src/state.rs:516-518:
```
    /// The TERM document governing this report: the thing a caller accepts to download
    /// any version of it. One per report, not per version — which is why a term grant
    /// minted through one version covers the others.
    pub term: Document,
```
The same rule in the data plane — src/state.rs:1641-1643:
```
            // A term document is the report's, so it is served under any version the
            // report published — and under none it did not.
            TERM_DOCUMENT_KIND => report.versions.get(&version).map(|_| &report.term),
```
and in the authorisation rule — src/state.rs:1615-1618:
```
    pub fn grant_covers(&self, token: &str, report: &Report) -> bool {
        self.term_tokens
            .get(token)
            .is_some_and(|grant| grant.term_digest == report.term.digest)
```
(`TermGrant.report_version` is explicitly inert — src/state.rs:705-709: "It is NOT what the grant authorises — the term above is — and no check reads it.")

The contradicting projection — src/state.rs:1829-1832:
```
pub fn term_arn(region: &str, report_id: &str, version: i64) -> String {
    let suffix = report_id.strip_prefix("report-").unwrap_or(report_id);
    format!("arn:aws:artifact:{region}::term/term-{suffix}-v{version}")
}
```
reached per version from `Report::to_detail` — src/state.rs:612: `            term_arn: Some(term_arn(region, &self.id, version.version)),` which is what the handler answers with — src/ops/get_report_metadata.rs:191-193: `        Ok(GetReportMetadataOutput { report_details: Some(report.to_detail(version, &region)), })`.

The model calls the member an identifier of a resource, not a locator: `ReportDetail$termArn` — "<p>Unique resource ARN for term resource.</p>".

Test coverage: tests/all/report_workflow.rs:386-398 pins the version-bearing string, with the stated reason "expected a service-owned term ARN" (i.e. the reason given is the empty account segment, not the `-v{version}` suffix). The seeded report `report-K7bNqS2fWx8LmT4p` (`acceptance_type: "EXPLICIT"`, src/state.rs:3277-3305) publishes versions 1 and 2, so it exhibits both halves.

## Steps to Reproduce

1. Boot the emulator from this checkout.
2. GET /v1/report/getMetadata?reportId=report-K7bNqS2fWx8LmT4p&reportVersion=1 and note reportDetails.termArn = arn:aws:artifact:<region>::term/term-K7bNqS2fWx8LmT4p-v1.
3. GET /v1/report/getMetadata?reportId=report-K7bNqS2fWx8LmT4p&reportVersion=2 and note reportDetails.termArn = ...-v2 — a second term ARN for the same report.
4. GET /v1/report/getTermForReport?reportId=report-K7bNqS2fWx8LmT4p&reportVersion=1 and keep the termToken it returns.
5. GET /v1/report/get?reportId=report-K7bNqS2fWx8LmT4p&reportVersion=2&termToken=<that token> — it is served, because `grant_covers` treats the report's term as one document, contradicting the two term ARNs step 2 and 3 reported.
