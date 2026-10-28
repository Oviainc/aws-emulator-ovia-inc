Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b056d947c24faab0377cbe866250e0547cccc526

# ListServiceQuotas re-implements crate::state::requested_at_level inline instead of calling it

_Filed: 2026-10-28T07:35:13Z_
_Operation: ListServiceQuotas | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b056d947c24faab0377cbe866250e0547cccc526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `AppliedLevelEnum` filter rule — "`ALL` covers every level, any other value keeps exactly that level, and a caller that names none gets `ACCOUNT`" — exists twice in this crate.

It lives once as the shared predicate `crate::state::requested_at_level`, which both change-history listings reach through `history_view`. `ListServiceQuotas` needs the same rule over the same enum (its `QuotaAppliedAtLevel` member carries the identical "ACCOUNT is the default" documentation as the history listings' `QuotaRequestedAtLevel`), but spells it out inline as two hand-written boolean guards instead of calling the helper.

The two spellings are presently equivalent for all eight (record level x requested level) combinations — `requested_at_level(LEVEL_ACCOUNT, level)` reproduces the first guard and `requested_at_level(LEVEL_RESOURCE, level)` the second — so this is a pure duplication with no behavioral difference today. That is exactly the drift surface: a change to the shared predicate (a new `AppliedLevelEnum` member, or a change of the no-filter default) silently fixes the two history listings and leaves `ListServiceQuotas` answering by the old rule, because nothing ties the inline copy to the helper.

The crate has already been through this once for the sibling listings: the fixed issue `20261027T222124Z-service-quotas-listrequestedservicequotachangehistory-maintainability` ("The history status/level filter-and-view chain is implemented twice, once per history listing") is what created `history_view`/`requested_at_level`. `ListServiceQuotas` is the remaining copy of the level half of that rule.

Fix: have the two guards in `list_service_quotas.rs` call `crate::state::requested_at_level(crate::state::LEVEL_ACCOUNT, level)` and `crate::state::requested_at_level(crate::state::LEVEL_RESOURCE, level)` (and widen the helper's doc, which today names only the history listings). Every response body and every existing test stays byte-identical.

## Evidence

The shared predicate — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:1998-2010:

    /// Whether a request made at `level` belongs in a history listing filtered to
    /// `requested`.
    ///
    /// `ALL` is the level that covers every other one, so it keeps the whole listing; any
    /// other value keeps the requests made at exactly that level. A caller that names no
    /// level gets [`LEVEL_ACCOUNT`], the documented default of the member on both history
    /// listings (`smithy_query(query="doc",
    /// shape="ListRequestedServiceQuotaChangeHistoryRequest",
    /// member="QuotaRequestedAtLevel")`: "ACCOUNT is the default").
    pub fn requested_at_level(level: &str, requested: Option<&str>) -> bool {
        let filter = requested.unwrap_or(LEVEL_ACCOUNT);
        filter == LEVEL_ALL || level == filter
    }

The one caller of it — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:1351-1356 (`history_view`), reached by both history handlers:

        requests
            .into_iter()
            .filter(|request| status.is_none_or(|status| request.status == status))
            .filter(|request| requested_at_level(&request.quota_requested_at_level, level))

The second copy of the same rule — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/list_service_quotas.rs:153-165:

            let level = input.quota_applied_at_level.as_deref();
            let mut quotas: Vec<ServiceQuota> = Vec::new();
            // ACCOUNT is the level this listing reports when the caller names none.
            if level.is_none_or(|level| {
                level == crate::state::LEVEL_ACCOUNT || level == crate::state::LEVEL_ALL
            }) {

and its other half at list_service_quotas.rs:163-165:

            if level.is_some_and(|level| {
                level == crate::state::LEVEL_RESOURCE || level == crate::state::LEVEL_ALL
            }) {

That the two filters are the same rule over the same enum, from the model — `smithy_query(query="shape", shape="ListServiceQuotasRequest")`, member `QuotaAppliedAtLevel`: "Filters the response to return applied quota values for the `ACCOUNT`, `RESOURCE`, or `ALL` levels. `ACCOUNT` is the default." — the identical sentence state.rs:2003-2006 cites for the history member.

Test coverage that must stay unchanged (and does, since the two spellings agree on every input):
* tests/all/quota_workflow.rs:542-575 `the_applied_level_filter_keeps_the_quotas_applied_at_that_level` — `let expectations = [("ACCOUNT", all.len()), ("ALL", all.len()), ("RESOURCE", 0)];`
* tests/all/resource_quota_context.rs:400-495 — the `RESOURCE`/`ALL`/no-filter planes, incl. line 481 "case no QuotaAppliedAtLevel: the listing defaults to the ACCOUNT plane"
* tests/all/applied_quota_listing_pages.rs:140-166 — the `QuotaAppliedAtLevel` narrowings across paged walks
* the history side is pinned by tests/all/history_filter_chain.rs and tests/all/requested_change_history.rs

## Steps to Reproduce

1. Open services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:2007 and read `requested_at_level`, the crate's single predicate for the AppliedLevelEnum filter (ALL keeps everything, no filter means ACCOUNT).
2. Open services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/list_service_quotas.rs:153-178 and observe the same rule written out again as `level.is_none_or(|l| l == LEVEL_ACCOUNT || l == LEVEL_ALL)` and `level.is_some_and(|l| l == LEVEL_RESOURCE || l == LEVEL_ALL)`, with no call to the helper.
3. Enumerate the eight combinations (record level ACCOUNT/RESOURCE x requested None/ACCOUNT/RESOURCE/ALL): `requested_at_level(LEVEL_ACCOUNT, level)` and `requested_at_level(LEVEL_RESOURCE, level)` give the same answers as the two inline guards, confirming the copies are redundant rather than different.
4. Simulate drift: change the no-filter default in `requested_at_level` (or add a level member) — the two history listings follow the helper while ListServiceQuotas keeps answering by its inline copy.
5. Replace the two guards with the helper calls and run `cargo test -p service-com-amazonaws-servicequotas-servicequotasv20190624`: quota_workflow.rs:544, resource_quota_context.rs:400-495 and applied_quota_listing_pages.rs still pass unchanged.
