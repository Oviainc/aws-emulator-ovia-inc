Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a

# The term grant's download receipt is minted from a sequential counter, so a caller that accepted no term can derive a valid termToken

_Filed: 2026-10-28T18:24:07Z_
_Operation: GetTermForReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate hands out two kinds of download receipt and states, in its own words, the rule a receipt has to follow: a value drawn from a counter authorises nobody, because a caller that was handed no URL can derive it. The inquiry-export plane follows that rule (its `download_token` is a v4 UUID, kept separate from the counter-minted export id that keys the store). The term grant does NOT: `GetTermForReport` mints the `termToken` through the shared counter helper (`mint_counter_id`, prefix `term`), so the token it answers with is exactly `term-{seq:016x}` — `term-0000000000000001` for the first acceptance in the process, `term-0000000000000002` for the second, and so on.

That same token is the ONLY thing that authorises a download of a report whose `acceptanceType` requires the explicit acceptance of its term: `GetReport` serves a presigned URL iff `store.download_authorised(&input.term_token, report)`, and the unsigned report document plane (which runs with `auth_schemes() -> Some(&[])`, i.e. no credential at all) decides the same way off the `termToken` query parameter. So a caller that never accepted the term — and, on the document plane, a caller that holds no credential and was handed no URL — can guess `term-0000000000000001` and get the gated document, which is precisely what the crate says a counter-minted value must not be able to do. The acceptance receipt stops being a receipt: the refusal the handler is written to give (`invalid_token()`, 400) is turned into a success.

The gap is not the capacity bound or the token's shape as a store KEY — the export plane shows the crate's own way out: keep the counter-minted id as the store key and mint the receipt that travels in the URL separately, so it cannot be derived.

## Evidence

The rule this crate states for a download receipt — services/service-com-amazonaws-artifact-artifact/src/state.rs:1707-1715:
```
/// Mint the token one export's document URL carries.
///
/// A receipt rather than a key into a store, so unlike [`mint_counter_id`] it is NOT drawn
/// from a counter: a counter-minted token is derivable by a caller that was handed no URL,
/// and a URL carrying only derivable values authorises nobody. It is a version-4 UUID's 32
/// hex digits, which satisfy the one query-value encoding [`QUERY_VALUE`] admits unescaped.
fn mint_inquiry_export_token() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}
```
and restated for the planes in services/service-com-amazonaws-artifact-artifact/src/service.rs:96-100: "The fetch is authorised by the download token the announced URL carries … the ONE thing a caller that was handed the URL holds — **the ids its path carries are minted from counters, so they authorise nobody** — and a fetch presenting no such token is refused `403`, as the report plane below refuses a fetch carrying no term grant."

The term grant breaks it — src/state.rs:1574-1596 (`mint_term_grant`, called from src/ops/get_term_for_report.rs:204 `let term_token = store.mint_term_grant(&report, resolved_version, now);`):
```
    pub fn mint_term_grant(
        &mut self,
        report: &Report,
        report_version: i64,
        now: SystemTime,
    ) -> String {
        let token = mint_counter_id(
            &mut self.term_tokens,
            &mut self.term_token_seq,
            TERM_TOKEN_PREFIX,
            Some(TERM_GRANT_CAPACITY),
        );
```
with the derivable value it produces at src/state.rs:1791-1797:
```
    loop {
        *seq = seq.wrapping_add(1);
        let candidate = format!("{prefix}-{:016x}", *seq);
        if !store.contains_key(&candidate) {
            return candidate;
        }
    }
```
and `TERM_TOKEN_PREFIX` at src/state.rs:1741-1744 (`const TERM_TOKEN_PREFIX: &str = "term";`), so the first grant of a process is always `term-0000000000000001`.

What that token alone decides — src/state.rs:1620-1622:
```
    pub fn download_authorised(&self, token: &str, report: &Report) -> bool {
        !report.requires_explicit_acceptance() || self.grant_covers(token, report)
    }
```
reached from src/ops/get_report.rs:202-226 ("The term token is the receipt of the acceptance a report's `AcceptanceType` REQUIRES … where the report requires that acceptance, a token that was never issued is not a token at all" → `return Err(GetReportErrorModeled::ValidationException(crate::state::invalid_token()).into());`), and from the UNSIGNED document plane at src/service.rs:435-451 (`let token = query_param(&query, crate::state::TERM_TOKEN_PARAM).unwrap_or_default(); let authorised = store.reports.get(&wanted.report_id).is_some_and(|report| store.download_authorised(&token, report));`) whose own auth is src/service.rs:477-479 `fn auth_schemes(&self) -> Option<&'static [&'static str]> { Some(&[]) }`.

No test covers the derivable token, and two suites show why: both pick the single counter value that is never minted (the counter is pre-incremented, so seq 0 never appears). tests/all/report_workflow.rs:621 `for bogus in ["bogus-token", "term-0000000000000000", "never_issued"]` asserts a 400, and tests/all/report_acceptance_type.rs:39-48 lists `("shaped like a minted token".to_string(), "term-0000000000000000".to_string())` among "Tokens no acceptance ever minted". tests/all/document_data_plane.rs:418-426 only exercises `no termToken`, `termToken=never-issued` and another report's token. Substituting `term-0000000000000001` in any of those rows turns the asserted refusal into a served document.

## Steps to Reproduce

1. Boot the emulator and call GET /v1/report/list to find a report whose `acceptanceType` is `EXPLICIT` (the suites' `a_report_requiring_acceptance()`).
2. As caller A, call GET /v1/report/getTermForReport?reportId=<that report> and note the answered `termToken`: it is `term-0000000000000001` on a fresh process (mint_counter_id pre-increments `term_token_seq` from 0 and formats `{prefix}-{:016x}`).
3. As caller B — which has accepted nothing — call GET /v1/report/get?reportId=<that report>&termToken=term-0000000000000001 without ever calling GetTermForReport. The guessed value satisfies `grant_covers`, so a 200 with a `documentPresignedUrl` is served where get_report.rs:217-225 is written to answer 400 `ValidationException` with `invalid_token()`.
4. Follow the announced URL (or construct /v1/report/download/report/<reportId>/v<version>/<object>?termToken=term-0000000000000001): the document plane, which requires no SigV4 at all (service.rs:477-479), serves the bytes on the same guessed token.
5. Contrast the sibling receipt: POST /v1/compliance-inquiry/export and note the `exportToken` in the announced URL is a 32-hex v4 UUID (state.rs:1713-1715) that no other caller can derive — the rule state.rs:1709-1712 and service.rs:96-100 state for a download receipt, and that the term grant does not follow.
