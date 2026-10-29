Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# A job's EndTime is stamped from the reading caller's clock, not from the instant the transition the record holds fell due

_Filed: 2026-10-29T03:24:52Z_
_Operation: DescribeFHIRExportJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every job record in this crate owes the clock ONE transition, whose DUE INSTANT is stored on the record (`pending: Option<(Instant, JobStatus, Option<String>)>`) and materialised by whichever call reads it next. When `settle` materialises a terminal status it writes `EndTime` from a FRESH `SystemTime::now()` instead of from the due instant the record already holds, so the reported `EndTime` is the instant somebody first LOOKED at the job, not the instant the job ended.

Two consequences a caller can observe:

1. The gap between `SubmitTime` and `EndTime` is not the job's run span but the caller's polling delay. A `StartFHIRExportJob` arms the transition at `Instant::now() + EXPORT_JOB_RUN` (500 ms), so the job ends at `SubmitTime + 500 ms`; a caller that describes it 30 s later is told `EndTime = SubmitTime + 30 s`.
2. Two jobs' `EndTime`s can INVERT their real end order, because the order is decided by who read first. Job A started at t=0 (ends t=0.5) and job B started at t=1 (ends t=1.5): describe B at t=2 (B.EndTime=2), then describe A at t=3 (A.EndTime=3) — A, which ended first, reports the later `EndTime`.

The field's own doc comment states the rule the code under it breaks: it says `ended_at` is "the instant the job reached a terminal status", and the body stamps the instant of the read. The same body is copied into all three job records, so `DescribeFHIRImportJob`/`ListFHIRImportJobs` and `DescribeDataTransformationJob`/`ListDataTransformationJobs` report the same thing; a fix should settle all three copies. No test pins the VALUE: the suites only assert that `EndTime` is present exactly for a terminal status and that two reads agree (`tests/all/fhir_export_job_describe.rs:761-768`), and every test that checks a concrete `EndTime` seeds the field directly (`tests/all/data_transformation_job.rs:430-434`).

## Evidence

MODEL — `smithy_query(query="doc", shape="ExportJobProperties", member="EndTime")`:
"The time the export job completed."

THE RECORD'S OWN FIELD DOC — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:1072-1074:
```
    /// `EndTime`: the instant the job reached a terminal status, written ONCE
    /// by [`ExportJob::settle`] so two reads of a finished job cannot differ.
    pub ended_at: Option<::aws_smithy_types::DateTime>,
```

THE CODE UNDER IT, stamping the READER's clock instead — src/state.rs:1116-1137 (the due instant taken out of `pending` at 1124 is discarded; `SystemTime::now()` at 1135):
```
    fn settle(&mut self, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_none_or(|(due, _, _)| now < *due)
        {
            return;
        }
        let Some((_, next, message)) = self.pending.take() else {
            return;
        };
        self.status = next;
        ...
        // `EndTime` is owed once, by the step that reaches a status the job
        // never leaves, and is then stored — never re-derived per read.
        if next.is_terminal() && self.ended_at.is_none() {
            self.ended_at = Some(::aws_smithy_types::DateTime::from(SystemTime::now()));
        }
    }
```

THE DUE INSTANT IS ON THE RECORD — src/state.rs:1061-1069:
```
    /// The next transition the clock owes this job, as `(due instant, next
    /// status, the message that status comes with)`. ...
    pub pending: Option<(Instant, JobStatus, Option<String>)>,
```
and it is armed a KNOWN span after `SubmitTime` — src/ops/start_fhir_export_job.rs:339-345:
```
            status: crate::state::JobStatus::Submitted,
            pending: Some((
                ::std::time::Instant::now() + crate::state::EXPORT_JOB_RUN,
                owed.0,
                owed.1,
            )),
            submitted_at: ::aws_smithy_types::DateTime::from(now),
```
with src/state.rs:88-97: "How long after the call that starts it a FHIR export job's pending status transition falls due. ... It is never slept on — the due instant is stored on the record and materialised by the next call that reads it."

THE SAME BODY COPIED — src/state.rs:1227-1231 (`ImportJob::settle`) and src/state.rs:1404-1408 (`TransformationJob::settle`), both `self.ended_at = Some(::aws_smithy_types::DateTime::from(SystemTime::now()));`.

TEST COVERAGE (presence and stability only, never the value) — tests/all/fhir_export_job_describe.rs:761-768:
```
    // `EndTime` is stored when the job ends, not re-derived per read, so a
    ...
        again["EndTime"], first["EndTime"],
        "two describes of one ended job must report the same EndTime, got {again} after {first}"
```

## Steps to Reproduce

1. Call `CreateFHIRDatastore` with `DatastoreTypeVersion: "R4"` and wait for `DescribeFHIRDatastore` to report `ACTIVE`.
2. Call `StartFHIRExportJob` with that `DatastoreId`, a `DataAccessRoleArn` and an `OutputDataConfig`; note the `JobId`.
3. Call `DescribeFHIRExportJob` immediately and record `ExportJobProperties.SubmitTime` (the job is still `SUBMITTED`, no `EndTime`).
4. Sleep well past the 500 ms `EXPORT_JOB_RUN` span — say 10 seconds — then call `DescribeFHIRExportJob` again.
5. Observe `EndTime - SubmitTime` is ~10 s rather than the ~0.5 s span the record's own pending transition was armed for, i.e. `EndTime` is the instant of this read.
6. For the order inversion: start job A, wait 1 s, start job B, wait 1 s, describe B (B gets EndTime=now), wait 1 s, describe A — A reports a LATER `EndTime` than B although A ended first.
