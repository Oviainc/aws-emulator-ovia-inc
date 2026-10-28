Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc

# A sequence NextToken is not scoped to the listing that minted it, so another listing's cursor silently empties (or restarts) the page

_Filed: 2026-10-28T23:05:11Z_
_Operation: ListCallAnalyticsJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All eight sequence-paged listings of this crate (ListCallAnalyticsJobs, ListLanguageModels, ListMedicalScribeJobs, ListMedicalTranscriptionJobs, ListMedicalVocabularies, ListTranscriptionJobs, ListVocabularies, ListVocabularyFilters) read their `NextToken` through the single-homed codec `crate::state::resume_after_sequence` -> `parse_page_cursor`. That codec validates only that the token's sequence is one the STORE has minted (`1..=minted_through`). But `minted_through` comes from `ComAmazonawsTranscribeTranscribeState::minted_sequences`, and `next_sequence` is ONE counter shared by every family's creates and starts — so a cursor minted by ListVocabularies passes the check unchanged when it is presented to ListTranscriptionJobs (or any other listing), and the `{sequence}|{name}` token's name half is parsed but deliberately ignored.

The consequence is the exact symptom the crate's own cursor-rule test says must not happen, in both directions:

* A cursor minted by a listing of an OLDER family, replayed against a newer family, resumes below every record of that family: the listing answers HTTP 200 with an EMPTY page and no NextToken although the Region holds records. The caller is told the collection is exhausted.
* A cursor minted by a listing of a NEWER family, replayed against an older one, resumes above every record of that family: the listing answers the FIRST page again plus a fresh cursor — "silently answering the FIRST page again", which the test header names as the failure mode the rule exists to prevent.

In neither case is the caller given the refusal the codec's own single-homed message promises, which names the operation whose previous answer the token should have come from.

## Evidence

The rule the crate's own cursor test states — services/service-com-amazonaws-transcribe-transcribe/tests/all/list_cursor_rule.rs:6-13:
```
//! THE RULE UNDER TEST: a listing resumes only from a `NextToken` this service itself
//! minted — the whole of what its cursor codec hands out (`crate::state::page_cursor`:
//! a creation sequence this store has actually minted, the separator, and the
//! non-empty record name it was minted with) — and refuses every other token with the
//! modelled `BadRequestException` naming the listing that would have handed one out,
//! instead of silently answering the FIRST page again. A client looping `while
//! NextToken` with a stale, truncated or foreign cursor is told its cursor is invalid
//! rather than re-reading page one forever.
```

Why the test never exercises a foreign cursor — list_cursor_rule.rs:130-134:
```
    for listing in sequence_paged() {
        // A fresh store per listing, so the sequences minted are exactly this
        // family's: 1..=minted, in creation order.
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
```
Every row gets a store holding ONE family's records, so no cursor of another listing ever exists to present.

What the codec actually accepts — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:3179-3188:
```
pub fn parse_page_cursor(token: &str, minted_through: u64) -> Option<u64> {
    let (sequence, name) = token.split_once(SEQUENCE_CURSOR_SEPARATOR)?;
    if name.is_empty() {
        return None;
    }
    sequence
        .parse::<u64>()
        .ok()
        .filter(|sequence| (1..=minted_through).contains(sequence))
}
```
The name is required to be non-empty and then discarded; nothing records which listing minted the sequence.

That the counter is shared by every family — state.rs:1629-1632 and state.rs:1642-1644:
```
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }
...
    pub fn minted_sequences(&self) -> u64 {
        self.next_sequence
    }
```
It is called by every create and start of all nine families (e.g. `let sequence = store.next_sequence();` at src/ops/create_vocabulary.rs:370 and at src/ops/start_transcription_job.rs:583), so sequences interleave across families.

The refusal text the codec single-homes, which states the per-operation scope — state.rs:3226-3231:
```
pub fn invalid_next_token_message(operation: &str) -> String {
    format!(
        "The NextToken in your request is not valid. Use the NextToken returned by \
         your previous {operation} request."
    )
}
```

The handler that reaches it, and what the accepted cursor then does — src/ops/list_call_analytics_jobs.rs:197-207 passes `store.minted_sequences()` into `resume_after_sequence` and feeds its result straight to `list_page` as `resume_below` (list_call_analytics_jobs.rs:216-225); `list_page` skips every record at or above it — state.rs:3402-3404:
```
        if resume_below.is_some_and(|sequence| record.sequence() >= sequence) {
            continue;
        }
```

Test coverage: tests/all/list_cursor_rule.rs covers only in-family tokens (`forged_tokens`, list_cursor_rule.rs:67-123, and the accepted sweep at list_cursor_rule.rs:192-227); tests/all/list_paging_rule.rs pages each listing over its own family. No test puts two families' records in one store and replays one listing's cursor against another, so the accepting behaviour is unpinned.

## Steps to Reproduce

1. Through one Region's endpoint, call CreateVocabulary twice (names "v1", "v2"; LanguageCode "en-US"; Phrases ["alpha"]) — these take creation sequences 1 and 2 from the shared counter.
2. Call StartCallAnalyticsJob twice (names "j1", "j2"; Media.MediaFileUri pointing at an object the emulator's S3 holds) — these take sequences 3 and 4.
3. Call ListVocabularies with {"MaxResults": 1}. Observed: one vocabulary and NextToken "2|v2" (state.rs:3161 `page_cursor`).
4. Call ListCallAnalyticsJobs with {"NextToken": "2|v2"} — a cursor ListCallAnalyticsJobs never handed out. Observed: HTTP 200 with CallAnalyticsJobSummaries == [] and no NextToken, because 2 <= minted_through (4) so the token is accepted and `resume_below = 2` skips both jobs (sequences 3 and 4).
5. Expected per tests/all/list_cursor_rule.rs:6-13 and state.rs:3226: HTTP 400 BadRequestException "The NextToken in your request is not valid. Use the NextToken returned by your previous ListCallAnalyticsJobs request."
6. Now the other direction: call ListCallAnalyticsJobs with {"MaxResults": 1} to obtain NextToken "4|j2", then call ListVocabularies with {"NextToken": "4|j2"}. Observed: HTTP 200 with the FIRST page of vocabularies again plus a fresh NextToken — the "silently answering the FIRST page again" outcome the test header says the rule prevents.
