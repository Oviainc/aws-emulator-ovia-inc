Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 66b81b2d38066f623e620cf9384f94dad963db50

# A completed transcription job that declared no MediaFormat reports none, although this crate already derives the format from the stored media and the Call Analytics family reports exactly that

_Filed: 2026-10-29T12:06:16Z_
_Operation: GetTranscriptionJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `66b81b2d38066f623e620cf9384f94dad963db50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MediaFormat` is modelled identically on both job families' output shapes — "The format of the input media file." for `TranscriptionJob$MediaFormat` and the same sentence for `CallAnalyticsJob$MediaFormat` — and this crate answers the two families two different ways.

The Call Analytics family DETERMINES it when the job completes, from the media URI the request named: `CallAnalyticsJobRecord::advance_clock` sets `self.media_format = media_format_of(&self.media)`.

The transcription family (and the medical transcription family) never determines anything: the record is stored with `media_format: input.media_format` and `TranscriptionJobRecord::advance_clock` / `MedicalTranscriptionJobRecord::advance_clock` touch every other member the service owns at completion (status, completion time, identified languages, transcript location, subtitle locations) but never `media_format`. So a job started with `Media.MediaFileUri = s3://bucket/dictation.wav` and no `MediaFormat` completes and reports NO `MediaFormat`, while a Call Analytics job over the same object reports `"wav"`.

The detected format is not unavailable on that path — the same handler already computes it. `StartTranscriptionJob` calls `crate::state::media_failure_reason(media_verdict, input.media_format.as_deref(), &input.media)`, which calls `media_format_of(media)` on that very record's media to decide whether the declared format is contradicted. The consequence of that rule is that for any COMPLETED job of this family the declared format and the detected format necessarily agree (a disagreement ends the job `FAILED` with `MEDIA_FORMAT_MISMATCH_FAILURE_REASON`), so filling the member in from `media_format_of` where the request declared nothing cannot contradict anything the crate already reports — it is the one case the crate leaves blank rather than the one case it cannot know.

This also contradicts the state module's own stated rule for service-determined members: a member is "reported where the request states what it is determined from, and withheld where only the media could say". The format is determined from the stored object's own NAME, which the request states — which is why `media_format_of` exists and why the module header calls a contradicted `MediaFormat` "the one per-media failure this emulator can establish without decoding audio". Contrast `MediaSampleRateHertz`, which the crate deliberately withholds for this family because "only the audio itself could answer" it.

Affected identically: `GetMedicalTranscriptionJob` (same two lines in `MedicalTranscriptionJobRecord`), and `StartTranscriptionJob`/`StartMedicalTranscriptionJob` echo whatever the request sent.

## Evidence

The sibling family DETERMINES it — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:4975 (`CallAnalyticsJobRecord::advance_clock`):
```
4975|        self.media_format = media_format_of(&self.media);
```
with the record's own field doc, state.rs:4913-4916:
```
4913| /// `MediaFormat`, the format the service determined the input audio is in; `None`
4914| /// until the job completes, and for a media URI that evidences no modelled format
4915| /// ([`media_format_of`]).
4916|     pub media_format: Option<String>,
```

The helper that determines it, state.rs:4489-4500 / 4504:
```
4489| /// The `MediaFormat` of the audio a job was given, as the media URI it was given
4490| /// evidences it — `None` for a URI that evidences no modelled format.
...
4496| /// the name it is stored under: a URI whose last path segment ends in one of the
4497| /// modelled `MediaFormat` extensions ([`MEDIA_FORMAT_VALUES`]) is reported as that
4498| /// format — `Media`'s own documented examples are spelled that way
4499| /// (`s3://DOC-EXAMPLE-BUCKET/my-media-file.flac`) — and a URI that ends in no such
4500| /// extension leaves the member absent rather than guessed.
4504| pub fn media_format_of(media: &Media) -> Option<String> {
```

The transcription family never calls it for the member — src/ops/start_transcription_job.rs:597 is the only assignment:
```
597|            media_format: input.media_format,
```
and `TranscriptionJobRecord::advance_clock` (state.rs:3968-4016) fills in `completion_time`, `status`, `materialise_identified_languages()`, `self.transcript` and `subtitles.subtitle_file_uris` and never assigns `self.media_format`; `MedicalTranscriptionJobRecord::advance_clock` (state.rs:4196-4223) likewise, with src/ops/start_medical_transcription_job.rs:470 `media_format: input.media_format,`.

Yet the SAME start path already has the detected format — src/ops/start_transcription_job.rs:472-476:
```
472|        let failure_due = crate::state::media_failure_reason(
473|            media_verdict,
474|            input.media_format.as_deref(),
475|            &input.media,
476|        );
```
and state.rs:4727-4733 / 4739-4744:
```
4727| /// A format MISMATCH is the one per-media failure this emulator can establish: the
4728| /// format of the stored media is what its name evidences ([`media_format_of`]), and the
4729| /// request states what the caller believes it to be, so the two can disagree. Judged
4730| /// only for a job whose media actually RESOLVED ([`MediaVerdict::Resolved`]) ...
4742|    let declared = declared?;
4743|    let detected = media_format_of(media)?;
4744|    (detected != declared).then(|| MEDIA_FORMAT_MISMATCH_FAILURE_REASON.to_string())
```

The crate's own rule for such members — state.rs:55-56 and :63-65:
```
  55| //! A member the service itself DETERMINES is reported where the request states
  56| //! what it is determined from, and withheld where only the media could say.
  63| //! ... — and the Call Analytics family determines its language,
  64| //! media format and sample rate the same way, from the request and this family's own
  65| //! constants (see [`CallAnalyticsJobRecord`]).
```

Test coverage: the Call Analytics half is pinned — tests/all/call_analytics_job_results.rs:149-154 (`assert_eq!(&job["MediaFormat"], expected_format, ...)`) over "Every value the model's `MediaFormat` enum carries" (:33-35). The transcription half is pinned by NOTHING: tests/all/media_object_resolution.rs is the suite that drives declared/undeclared formats (`FormatCase.declared` — ":478 /// The declared `MediaFormat`, or `None` for a request that declares none."), and its only row with `declared: None` (":524-532 \"a medical transcription job declaring no format at all, so there is nothing for the stored media to contradict\"") asserts just `TranscriptionJobStatus == "COMPLETED"` and a null `FailureReason` (:566-589) — never that `MediaFormat` is absent. No existing expectation has to change.

## Steps to Reproduce

1. Assemble the emulator with transcribe and Amazon S3 on one engine, as tests/all/media_object_resolution.rs does, and PutObject s3://example-media/dictation.wav
2. POST / with X-Amz-Target: Transcribe.StartTranscriptionJob and body {"TranscriptionJobName":"no-format","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://example-media/dictation.wav"}} — no MediaFormat member
3. Sleep past crate::state::JOB_TRANSITION_DELAY (500 ms), then POST / with X-Amz-Target: Transcribe.GetTranscriptionJob {"TranscriptionJobName":"no-format"}
4. The answer reports TranscriptionJobStatus COMPLETED and carries NO MediaFormat, although the stored object's own name (dictation.wav) evidences the modelled format `wav` and crate::state::media_format_of already reads exactly that
5. POST / with X-Amz-Target: Transcribe.StartCallAnalyticsJob {"CallAnalyticsJobName":"no-format-ca","Media":{"MediaFileUri":"s3://example-media/dictation.wav"}}, wait the same span and GetCallAnalyticsJob — the same media under the sibling family reports "MediaFormat": "wav" (state.rs:4975)
6. Repeat step 2 with "MediaFormat":"mp3" against the same .wav object: the job settles FAILED with "The media format provided does not match the detected media format" — proving the detected format is established for this family too, and that a COMPLETED job of it can only ever have the detected format
