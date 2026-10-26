Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6eb5dfdd8fc0f77b54a569b75c45f405ceb2f53c

# The sequence page cursor accepts a NextToken this service never handed out and silently restarts the listing

_Filed: 2026-10-26T06:33:25Z_
_Operation: ListTranscriptionJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `6eb5dfdd8fc0f77b54a569b75c45f405ceb2f53c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every sequence-paged listing of this crate (ListTranscriptionJobs, ListMedicalTranscriptionJobs, ListMedicalScribeJobs, ListCallAnalyticsJobs, ListVocabularies, ListMedicalVocabularies, ListVocabularyFilters, ListLanguageModels) reads its `NextToken` through the single-homed codec `crate::state::resume_after_sequence` -> `parse_page_cursor`. The codec HANDS OUT `{sequence}|{name}` with a non-empty name (`page_cursor`), but when READING it only takes the text before the first `|`, parses it as `u64` and rejects `0`. So a token this service never wrote — a bare number such as `"7"`, or any positive integer at all, e.g. `"999999"` — is accepted as a cursor.

The consequence is exactly the one the codec's own documentation says it prevents: with `NextToken="999999"` no stored record has a sequence `>= 999999`, so nothing is skipped and the listing answers HTTP 200 with the FIRST page again (plus a fresh NextToken), instead of the modelled `BadRequestException`. A client looping `while NextToken` with a stale, truncated or foreign token re-reads page one indefinitely rather than being told its cursor is invalid.

The name-keyed sibling codec in the same module does validate the format it hands out: `parse_name_page_cursor` requires the `name|` prefix AND a non-empty name, so `ListCallAnalyticsCategories` refuses a token it never wrote while the eight sequence-paged listings do not.

## Evidence

The codec's own documented rule — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:2219-2223:
```
/// The creation sequence a `NextToken` resumes after, or `None` for a token this
/// service never handed out — which a listing refuses rather than silently restarting
/// from the top.
///
/// Creation sequences start at 1, so a cursor of 0 is one this service never wrote.
```
What it hands out — state.rs:2210-2217: "this service writes `{sequence}|{name}` and reads back the sequence ([`parse_page_cursor`])" / `pub fn page_cursor(sequence: u64, name: &str) -> String { format!("{sequence}|{name}") }`.

What it accepts — state.rs:2224-2230:
```
pub fn parse_page_cursor(token: &str) -> Option<u64> {
    token
        .split('|')
        .next()
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0)
}
```
No check that the `|` separator or the name was ever there, and no check that the sequence is one this store ever minted, so `"7"` and `"999999"` both resolve to `Some(..)`.

The strict sibling in the SAME module — state.rs:2232-2234 and 2257-2261:
```
/// The prefix a name-keyed page cursor carries, which is what tells a token this
/// service handed out from one it never wrote. See [`name_page_cursor`].
const NAME_CURSOR_PREFIX: &str = "name|";
...
pub fn parse_name_page_cursor(token: &str) -> Option<&str> {
    token
        .strip_prefix(NAME_CURSOR_PREFIX)
        .filter(|name| !name.is_empty())
}
```

The handler that reaches it — services/service-com-amazonaws-transcribe-transcribe/src/ops/list_transcription_jobs.rs:180-193: "// A page cursor ... read through this service's own token codec (`state::resume_after_sequence`), so every listing pages the same way and refuses a cursor this service never handed out in the same words." followed by `crate::state::resume_after_sequence(input.next_token.as_deref(), Self::NAME, ..)?`, whose `Ok(Some(sequence))` is passed straight to `list_page` as `resume_below` (list_transcription_jobs.rs:212-221); `list_page` skips only records with `record.sequence() >= sequence` (state.rs:2439-2441), so an out-of-range cursor skips nothing.

Test coverage leaves the hole open: services/service-com-amazonaws-transcribe-transcribe/tests/all/list_paging_rule.rs:458-471 states the rule — "async fn every_listing_refuses_a_cursor_it_did_not_hand_out" / "// Creation sequences start at 1 and every record name is non-empty, so none of these is a cursor this service ever wrote — in either of its two token formats. Listing from the top instead would hide a caller's paging bug." — but its token table is `["not-a-cursor", "0|absent", "-1|absent", "name|", "|", "99999999999999999999999999"]`: every row either fails to parse as `u64` (the 26-digit one overflows) or is `<= 0`. The same gap is in tests/all/transcription_jobs.rs:750-754 (`["not-a-cursor", "0|demo-job", "|", "-1|demo-job"]`). No test sends a bare, in-range numeric token, so nothing pins the accepting behaviour.

## Steps to Reproduce

1. StartTranscriptionJob twice (e.g. names "job-a" and "job-b", LanguageCode="en-US", Media.MediaFileUri="s3://bucket/a.flac") so the Region holds records with sequences 1 and 2.
2. Call ListTranscriptionJobs with {"NextToken": "999999"} — a value this service never handed out (its cursors are always `{sequence}|{name}`).
3. Observed: HTTP 200 with TranscriptionJobSummaries listing both jobs — the first page again — because resume_below=999999 skips no record (state.rs:2439).
4. Expected per state.rs:2219-2221 and the rule tests/all/list_paging_rule.rs:458 asserts: HTTP 400 BadRequestException with "The NextToken in your request is not valid. Use the NextToken returned by your previous ListTranscriptionJobs request."
5. Repeat with {"NextToken": "2"} (bare sequence, no `|name`): also accepted (returns the record with sequence 1), where ListCallAnalyticsCategories refuses the analogous prefix-less token via parse_name_page_cursor.
