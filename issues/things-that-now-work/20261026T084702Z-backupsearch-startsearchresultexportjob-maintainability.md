Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# The ClientToken replay rule (live-token lookup, then fingerprint compare, then conflict) is implemented twice in this crate

_Filed: 2026-10-26T08:47:02Z_
_Operation: StartSearchResultExportJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's two `Start` operations each spell out the whole ClientToken replay rule themselves, and the state module spells out the live-token lookup twice as well. The rule has three steps — find a held record carrying this token still inside its validity window, answer the stored record when the request fingerprint matches, answer ConflictException when it does not — and both copies are byte-for-byte the same logic with only the record type, the operation name in the message and the resource-type constant differing. The two copies can drift: a change to the replay rule (which step runs first, what a non-matching fingerprint answers, whether a closed window falls through) has to be made twice or the two operations stop agreeing about what idempotency means, even though the model states the SAME window sentence on both ClientToken members.

This is the one duplication left in a crate that otherwise single-homes exactly this kind of rule: the two job listings share `newest_first`, the two identifier mints share `fresh_identifier`, the three tag writes share `check_tag_map`, and both replay checks already share `request_fingerprint` and `ClientToken::is_live`. Hoisting the replay branch (and the live-token lookup) the same way leaves every response and every existing test unchanged — the differing message text and resource type are parameters of the refusal, not of the rule.

## Evidence

Copy 1 — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_job.rs:220-238:
```
        if let Some(token) = input.client_token.as_deref() {
            if let Some(job) = state.job_by_client_token(token, now) {
                if job.request_fingerprint != fingerprint {
                    return Err(StartSearchJobErrorModeled::ConflictException(ConflictException {
                        message: format!(
                            "ClientToken '{token}' was already used for a different StartSearchJob request"
                        ),
                        resource_id: job.identifier.clone(),
                        resource_type: SEARCH_JOB_RESOURCE.to_string(),
                    })
                    .into());
                }
                return Ok(StartSearchJobOutput { ... });
            }
        }
```
Copy 2 — src/ops/start_search_result_export_job.rs:251-271:
```
        if let Some(token) = input.client_token.as_deref() {
            if let Some(job) = state.export_job_by_client_token(token, now) {
                if job.request_fingerprint != fingerprint {
                    return Err(StartSearchResultExportJobErrorModeled::ConflictException(
                        ConflictException {
                            message: format!(
                                "ClientToken '{token}' was already used for a different \
                                 StartSearchResultExportJob request"
                            ),
                            resource_id: job.identifier.clone(),
                            resource_type: EXPORT_JOB_RESOURCE.to_string(),
                        },
                    )
                    .into());
                }
                return Ok(StartSearchResultExportJobOutput { ... });
            }
        }
```
And the lookup behind them, also twice — src/state.rs:955-959:
```
    pub fn job_by_client_token(&self, token: &str, now: SystemTime) -> Option<&SearchJob> {
        self.search_jobs
            .values()
            .find(|job| job.client_token.as_ref().is_some_and(|held| held.is_live(token, now)))
    }
```
vs src/state.rs:1014-1018:
```
    pub fn export_job_by_client_token(&self, token: &str, now: SystemTime) -> Option<&ExportJob> {
        self.export_jobs
            .values()
            .find(|job| job.client_token.as_ref().is_some_and(|held| held.is_live(token, now)))
    }
```

The crate's own standard for exactly this situation — src/state.rs:818-836 (`newest_first`): "ONE helper behind BOTH job listings of this service ... so the two cannot drift apart on the order the service reports its jobs in"; src/state.rs:1045-1047 (`fresh_identifier`): "One mint for every identifier this service hands out, so two of them cannot disagree about the shape of an id"; src/state.rs:1579-1582 (`check_tag_map`): "This is the one admission rule every operation that writes a tag applies ... so the per-entry check, the count that is checked, the order of the two checks and the refusal channel are decided here once instead of per handler"; and src/state.rs:1463-1471 (`request_fingerprint`): "One helper for every `Start` operation, so two idempotency checks cannot disagree about what 'the same body' means."

Existing tests that cover both copies and must stay green unchanged: tests/all/client_token_window.rs (`a_repeat_under_a_search_job_client_token_is_the_same_job_only_inside_the_window`, `a_different_body_under_a_search_job_client_token_conflicts_only_inside_the_window`, `an_export_job_client_token_follows_the_same_window`), tests/all/export_client_token_expiry.rs (`a_repeat_served_from_inside_the_window_does_not_extend_it`, `past_the_window_the_export_job_the_token_mints_takes_over_idempotency`), tests/all/search_jobs.rs:746 and tests/all/export_jobs.rs:604 (`a_client_token_is_idempotent_for_the_same_body_and_conflicts_for_a_different_one`).

## Steps to Reproduce

1. Read src/ops/start_search_job.rs:220-238 and src/ops/start_search_result_export_job.rs:251-271 side by side: the same three-step replay rule, differing only in record type, operation name in the message and resource-type constant.
2. Read src/state.rs:955-959 and src/state.rs:1014-1018: the same live-token `values().find(...is_live(token, now))` lookup written twice, while the analogous pair of listings was hoisted into `newest_first` (src/state.rs:828-836) and the two mints into `fresh_identifier` (src/state.rs:1054-1062).
3. Change the replay rule in one copy only (e.g. return the stored record before comparing fingerprints) and observe that the other operation keeps the old rule, with nothing in the crate forcing the two to agree.
