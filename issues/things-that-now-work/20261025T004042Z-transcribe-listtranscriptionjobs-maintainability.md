Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# The cursor/filter/page-size/next-token loop is copied verbatim into seven list handlers

_Filed: 2026-10-25T00:40:41Z_
_Operation: ListTranscriptionJobs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Seven list handlers each carry their own copy of the same paging computation: skip records at or above the resumed sequence, apply the operation's filters, stop when the page is full, and emit a `page_cursor` built from the last record placed on the page. The copies differ only in the record type, the projection called (`to_summary()` / `to_info()`) and the name of the output member; the rule itself — including the subtle part, that `next_token` is emitted only when a FURTHER MATCHING record was found after the page filled, and is built from `last_on_page` rather than from the record that overflowed — is written out seven times and can drift in any one of them.

The same seven files also each carry a private `parse_next_token` whose body is identical apart from the operation name embedded in the refusal message.

The crate already single-homes the token codec (`crate::state::page_cursor` / `parse_page_cursor`, documented "Single-homed so every listing of this service pages the same way", `src/state.rs:1768`); the loop that uses it was not. A generic helper in `crate::state` taking the ordered records, the resume cursor, the page size, a predicate and a projection would leave every response byte-identical and every existing listing test unchanged.

## Evidence

Copy 1 — `src/ops/list_transcription_jobs.rs:212-241`:
```
        let mut summaries: Vec<TranscriptionJobSummary> = Vec::new();
        let mut next_token = None;
        let mut last_on_page: Option<(u64, String)> = None;
        for record in records {
            if resume_below.is_some_and(|sequence| record.sequence >= sequence) { continue; }
            ... filters ...
            if summaries.len() == page_size {
                next_token = last_on_page.as_ref()
                    .map(|(sequence, name)| crate::state::page_cursor(*sequence, name));
                break;
            }
            summaries.push(record.to_summary());
            last_on_page = Some((record.sequence, record.name.clone()));
        }
```
Copy 2 — `src/ops/list_medical_transcription_jobs.rs:214-243` (same ten statements, `MedicalTranscriptionJobSummary`).
Copy 3 — `src/ops/list_call_analytics_jobs.rs:212-241` (`CallAnalyticsJobSummary`).
Copy 4 — `src/ops/list_medical_scribe_jobs.rs:213-242` (`MedicalScribeJobSummary`).
Copy 5 — `src/ops/list_vocabularies.rs:207-236` (`VocabularyInfo`, `state_equals` filter).
Copy 6 — `src/ops/list_medical_vocabularies.rs:210-239` (`VocabularyInfo`, `state_equals` filter).
Copy 7 — `src/ops/list_vocabulary_filters.rs:198-220` (`VocabularyFilterInfo`, name filter only).

The seven `parse_next_token` copies: `list_transcription_jobs.rs:259-270`, `list_medical_transcription_jobs.rs:261-272`, `list_call_analytics_jobs.rs:259-270`, `list_medical_scribe_jobs.rs:260-271`, `list_vocabularies.rs:255-266`, `list_medical_vocabularies.rs:258-269`, `list_vocabulary_filters.rs:235-246` — each `crate::state::parse_page_cursor(token).ok_or_else(|| ...BadRequestException{...})` with only the operation name differing.

The single-homing intent the loop misses is stated in `src/state.rs:1761-1771`: "The token is opaque to clients: this service writes `{sequence}|{name}` and reads back the sequence ([`parse_page_cursor`]). ... Single-homed so every listing of this service pages the same way."

Tests that pin the behaviour these copies must keep: `tests/all/transcription_jobs.rs` and `tests/all/medical_transcription_job_listing.rs` / `tests/all/medical_scribe_job_listing.rs` listing suites, `tests/all/call_analytics_jobs.rs:488` (`the_listing_is_every_stored_job_newest_first_paged_and_filtered`), `tests/all/call_analytics_jobs.rs:663` (`a_next_token_this_service_did_not_hand_out_is_refused`), `tests/all/vocabularies.rs:500` (`listing_vocabularies_filters_and_pages`), `tests/all/medical_vocabularies.rs:384` and `:537` (`listing_medical_vocabularies_is_newest_first_and_honours_every_filter`, `listing_medical_vocabularies_pages_with_its_own_token`).

## Steps to Reproduce

1. Open `src/ops/list_transcription_jobs.rs:212-241` and `src/ops/list_medical_transcription_jobs.rs:214-243` side by side: the ten-statement paging loop is identical apart from the summary type
2. Repeat with `list_call_analytics_jobs.rs:212-241`, `list_medical_scribe_jobs.rs:213-242`, `list_vocabularies.rs:207-236`, `list_medical_vocabularies.rs:210-239`, `list_vocabulary_filters.rs:198-220` — seven copies of the same rule, including the `next_token = last_on_page...` cursor-emission subtlety
3. Compare the seven private `parse_next_token` helpers (e.g. `list_vocabularies.rs:255-266` vs `list_vocabulary_filters.rs:235-246`): identical bodies, only the operation name in the message differs
4. Confirm the listing tests named in the evidence pass unchanged before and after hoisting the loop into one `crate::state` helper
