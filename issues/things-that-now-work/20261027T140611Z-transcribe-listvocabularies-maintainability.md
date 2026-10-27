Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f39310ffb44e85959dc0d40e62557237e3e92192

# The "materialise clock-owed transitions, then order newest-first" rule is copied once per family in state.rs instead of written once over the traits the crate already has

_Filed: 2026-10-27T14:06:11Z_
_Operation: ListVocabularies | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f39310ffb44e85959dc0d40e62557237e3e92192`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` implements the same two rules seven times each, once per resource family, even though this crate already contains the trait machinery that makes them generic.

Rule 1 — the single-record lookup: `get_mut(region, name)` then `record.advance_clock(now)` then `Some(record)`. Seven byte-identical copies: `transcription_job_mut`, `medical_transcription_job_mut`, `medical_scribe_job_mut`, `call_analytics_job_mut`, `vocabulary_mut`, `medical_vocabulary_mut`, `language_model_mut`.

Rule 2 — the listing projection: advance every record of the Region, collect, then `sort_by_key(|record| std::cmp::Reverse(record.sequence))`. Seven copies: `jobs_newest_first`, `medical_transcription_jobs_newest_first`, `medical_scribe_jobs_newest_first`, `call_analytics_jobs_newest_first`, `vocabularies_newest_first`, `medical_vocabularies_newest_first`, `language_models_newest_first` (plus `vocabulary_filters_newest_first`, which is the same body minus the clock step the filter family has no transition for).

The copies can drift: the clock step, the ordering key or the Region scoping can be changed in one family and forgotten in the other six, and each copy is a place a future family's lookup can silently skip `advance_clock` — exactly the defect the FIXED issue `20261026T042405Z-transcribe-gettranscriptionjob-maintainability` ("the only one whose clock-materialising lookup is hand-rolled") was about, now reproduced across families instead of across handlers.

The crate already states this exact principle and already does it for the sibling rule: `jobs_in_flight` is written ONCE, generic over `RegionalStore<R>` where `R: JobLifecycle`, and `ListedRecord` already gives all seven record types the `sequence()` accessor each copy spells inline as `record.sequence`. A generic `fn newest_first<R>(store: &mut RegionalStore<R>, region, now)` and `fn record_mut<R>(store, region, name, now)` over the same traits (adding the three one-line `materialise_due_transitions` impls the non-job families lack) replace all fourteen bodies. Every response and every existing test stays unchanged — the operations keep calling a helper with the same signature and the same result.

Operation named is `ListVocabularies` because its handler calls one of the duplicated copies (`vocabularies_newest_first`); the finding is in the shared state module and covers all of them.

## Evidence

The crate's own stated principle, services/service-com-amazonaws-transcribe-transcribe/src/state.rs:157-162:
```
/// The two lifecycle facts every job family's record holds: the status it is in, and
/// the clock-owed transitions that status still has to catch up with.
///
/// One trait rather than four copies of [`jobs_in_flight`], which is what every
/// family's concurrency quota is measured against: the four job records differ in
/// what a job IS, not in how its status moves.
pub trait JobLifecycle {
```
and the generic it enables, state.rs:242-255:
```
pub fn jobs_in_flight<R: JobLifecycle>(
    jobs: &mut RegionalStore<R>,
    region: &str,
    now: SystemTime,
) -> usize {
    let mut in_flight = 0;
    for record in jobs.values_in_mut(region) {
        record.materialise_due_transitions(now);
```

Copy 1 of the listing rule — state.rs:1512-1524:
```
    pub fn jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&TranscriptionJobRecord> {
        for record in self.transcription_jobs.values_in_mut(region) {
            record.advance_clock(now);
        }
        let mut records: Vec<&TranscriptionJobRecord> =
            self.transcription_jobs.values_in(region).collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }
```
Copy 2, state.rs:1546-1558 (identical modulo the store field and record type):
```
    pub fn medical_transcription_jobs_newest_first(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<&MedicalTranscriptionJobRecord> {
        for record in self.medical_transcription_jobs.values_in_mut(region) {
            record.advance_clock(now);
        }
        let mut records: Vec<&MedicalTranscriptionJobRecord> =
            self.medical_transcription_jobs.values_in(region).collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        records
    }
```
Copy 6, state.rs:1676-1688 (`medical_vocabularies_newest_first`) and copy 5, state.rs:1646-1656 (`vocabularies_newest_first`) have the same three statements again.

Copy 1 of the lookup rule — state.rs:1495-1504:
```
    pub fn transcription_job_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut TranscriptionJobRecord> {
        let record = self.transcription_jobs.get_mut(region, name)?;
        record.advance_clock(now);
        Some(record)
    }
```
Copy 7, state.rs:1701-1710:
```
    pub fn language_model_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut LanguageModelRecord> {
        let record = self.language_models.get_mut(region, name)?;
        record.advance_clock(now);
        Some(record)
    }
```
(the four remaining copies: state.rs:1532-1541, 1565-1574, 1598-1607, 1628-1637, 1661-1670.)

The accessor a generic version needs already exists for all seven types, state.rs:2686-2733:
```
pub trait ListedRecord {
    fn sequence(&self) -> u64;
...
listed_records! {
    TranscriptionJobRecord, status = status;
    MedicalTranscriptionJobRecord, status = status;
    MedicalScribeJobRecord, status = status;
    CallAnalyticsJobRecord, status = status;
    LanguageModelRecord, status = status;
    VocabularyRecord, status = state;
    MedicalVocabularyRecord, status = state;
    VocabularyFilterRecord;
}
```

Existing test coverage that must stay green and unchanged: tests/all/transcription_jobs.rs, tests/all/transcription_job_queueing.rs, tests/all/medical_transcription_job_listing.rs, tests/all/medical_scribe_job_listing.rs, tests/all/call_analytics_jobs.rs, tests/all/call_analytics_job_queueing.rs, tests/all/vocabularies.rs, tests/all/medical_vocabularies.rs, tests/all/language_model_listing.rs, tests/all/list_paging_rule.rs, tests/all/list_cursor_rule.rs, tests/all/state_roundtrip.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-transcribe-transcribe/src/state.rs and read lines 1487-1730: seven `*_mut` lookups each spelling `get_mut(region, name)? ; record.advance_clock(now) ; Some(record)`, and seven `*_newest_first` helpers each spelling the advance-all / collect / `sort_by_key(Reverse(sequence))` trio.
2. Compare them with state.rs:242-255, where the sibling rule (`jobs_in_flight`) is written once, generic over `RegionalStore<R>` with `R: JobLifecycle`, as the trait's own doc at state.rs:157-162 argues it should be ("One trait rather than four copies").
3. Confirm the generic version needs nothing new on the record side: `ListedRecord` at state.rs:2686-2733 already supplies `sequence()` for all seven record types, and `JobLifecycle::materialise_due_transitions` already wraps `advance_clock` for the four job families.
4. Note the drift risk: changing the ordering key or the clock step (or adding a family) requires editing up to fourteen bodies, and a copy that omits `advance_clock` reports a stale status — the defect already fixed once for this crate in 20261026T042405Z-transcribe-gettranscriptionjob-maintainability.
5. Fold the fourteen bodies into one generic lookup and one generic listing helper; run `cargo test -p service-com-amazonaws-transcribe-transcribe` and confirm every listed test still passes with byte-identical responses.
