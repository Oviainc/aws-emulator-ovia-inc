Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# A job naming a custom MEDICAL vocabulary that is not READY is accepted, on a stale justification the crate's own build has contradicted

_Filed: 2026-10-28T16:46:54Z_
_Operation: StartMedicalTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartMedicalTranscriptionJob` reaches the crate's single reference check (`ComAmazonawsTranscribeTranscribeState::unheld_reference_error`) for `Settings.VocabularyName`, exactly as the sibling starts do for their own references. That helper has TWO halves: the resource must be HELD, and the resource must be in a state a job can apply it in (`unusable_reference_state` -> `resource_not_usable_message`). The state half is written for ONE family only — the match arm covers `ResourceFamily::Vocabulary` and falls through to `_ => None` for everything else — so a medical transcription job naming a custom medical vocabulary that is still `PENDING`, or that ended `FAILED` because its table file was never read, is accepted and later reports `COMPLETED` against a vocabulary that was never built.

The skip is deliberate in the code, but its stated reason is no longer true. `unusable_reference_state`'s own doc justifies it with "a custom MEDICAL vocabulary ... is built only from a location this emulator reads nothing from, so no record of that family ever reaches a usable state". Two other places in the same crate say the opposite: `MedicalVocabularyRecord`'s own doc in the same file states the family's build reads the file from this emulator's Amazon S3 and settles `READY` when the store serves it, and `CreateMedicalVocabulary::handle` does exactly that read before storing the record. So the premise ("no record of that family ever reaches a usable state", therefore refusing would refuse requests the real service serves) is false: with the state check in place only a medical vocabulary that genuinely is not `READY` would be refused, which is precisely the rule the non-medical family already applies with the very same helper and the very same message.

Effect: the two custom-vocabulary families answer differently for identical inputs. `StartTranscriptionJob` naming a `PENDING`/`FAILED` custom vocabulary is refused `BadRequestException` with `resource_not_usable_message`; `StartMedicalTranscriptionJob` naming a `PENDING`/`FAILED` custom MEDICAL vocabulary is answered 200 and a job is stored. The fix is one match arm (the family's `medical_vocabulary_mut` lookup already exists and already materialises the clock-owed transition, and `ResourceFamily::MedicalVocabulary::kind()` already yields the wording). Note for the fixer: `tests/all/medical_transcription_jobs.rs` creates its vocabularies and starts the jobs immediately, i.e. while they are still `PENDING`, so that suite will need the same "wait until the named vocabulary has settled" step the non-medical suite (`tests/all/job_start_vocabulary_state.rs`) already uses.

## Evidence

The skip and its stated reason — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1807-1812 (doc of `unusable_reference_state`):
"/// * a custom MEDICAL vocabulary ([`MedicalVocabularyRecord`]) is built only from a
///   location this emulator reads nothing from, so no record of that family ever
///   reaches a usable state. Refusing every job that names one would refuse a
///   request the real service serves — a limitation of this emulator's build, not
///   an error in the caller's request — so a held medical vocabulary is applied as
///   named;"

and the code it justifies — state.rs:1814-1828:
"    fn unusable_reference_state(... ) -> Option<String> {
        match family {
            ResourceFamily::Vocabulary => {
                let record = self.vocabulary_mut(region, name, now)?;
                (record.state != VOCABULARY_STATE_READY).then(|| record.state.clone())
            }
            _ => None,
        }
    }"

The SAME file contradicting that premise — state.rs:2168-2174 (doc of `MedicalVocabularyRecord`):
"/// holds an Amazon S3 of its own, so the file is READ from it in process
/// ([`resolve_vocabulary_file`]) and the table's lines are the terms the build
/// produces. The build therefore ends the way the object store's answer decides:
/// `PENDING` at the create, then [`VOCABULARY_STATE_READY`] — with those terms stored
/// at the `DownloadUri` the reader reports ([`store_vocabulary_content`]) — for a file
/// the store served, and [`VOCABULARY_STATE_FAILED`] with a grounded `FailureReason`
/// only for one it did not"

and the handler that performs that read — src/ops/create_medical_vocabulary.rs:186-193 and :237-243:
"/// Amazon S3 object the caller uploaded before calling, and this emulator holds an
/// Amazon S3 of its own, so the build READS it in process
/// ([`crate::state::resolve_vocabulary_file`]): a file the object store serves
/// settles the vocabulary `READY` once the clock makes its build due"
"        let verdict = crate::state::resolve_vocabulary_file(
            ctx,
            &region,
            crate::state::VOCABULARY_FILE_MEMBER,
            &input.vocabulary_file_uri,
        )
        .await;"

The call site that therefore gets only the existence half — src/ops/start_medical_transcription_job.rs:382-394:
"        if let Some(message) = store.unheld_reference_error(
            &region,
            now,
            &[(
                crate::state::ResourceFamily::MedicalVocabulary,
                input
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.vocabulary_name.as_deref()),
            )],
        ) {"

The sibling rule the same helper applies for the other family — state.rs:1761-1765 and :1784-1786:
"/// A resource that IS held must also be one a job can apply: a custom vocabulary
/// carries a `VocabularyState` precisely so that only a build that finished
/// successfully is usable (\"If the state is READY, you can use the custom
/// vocabulary\"), so one still being built or one whose build failed is refused with
/// [`resource_not_usable_message`]"
"            if let Some(state) = self.unusable_reference_state(*family, region, name, now) {
                return Some(resource_not_usable_message(family.kind(), name, &state));
            }"

Model support — smithy_query(query="doc", shape="CreateMedicalVocabularyResponse", member="VocabularyState"): "The processing state of your custom medical vocabulary. If the state is READY, you can use the custom vocabulary in a StartMedicalTranscriptionJob request."

Test coverage: nothing pins the current behaviour. tests/all/job_start_vocabulary_state.rs:50-52 scopes its table to "Every member path of every start operation that names a custom vocabulary of the (non-medical) custom vocabulary family" and lists no `StartMedicalTranscriptionJob` row; tests/all/medical_transcription_jobs.rs:159-162 creates the named vocabularies only so the EXISTENCE check passes ("a start may only name a custom medical vocabulary this account created") and asserts nothing about its state.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateMedicalVocabulary and body {"VocabularyName":"clinic-terms","LanguageCode":"en-US","VocabularyFileUri":"s3://DOC-EXAMPLE-BUCKET/absent-table.txt"} — the create answers 200 with VocabularyState PENDING
2. Immediately POST / with X-Amz-Target: Transcribe.StartMedicalTranscriptionJob and body {"MedicalTranscriptionJobName":"pending-vocab","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://example-media/consult.wav"},"OutputBucketName":"out","Specialty":"PRIMARYCARE","Type":"CONVERSATION","Settings":{"VocabularyName":"clinic-terms"}} — answered 200 and the job is stored, although the named vocabulary is still PENDING
3. Wait past the vocabulary's transition (the file was never served, so GetMedicalVocabulary now reports FAILED) and repeat the start with job name "failed-vocab" — answered 200 again
4. Compare the non-medical family: CreateVocabulary {"VocabularyName":"terms","LanguageCode":"en-US","Phrases":["a"]} followed immediately by StartTranscriptionJob with Settings.VocabularyName "terms" is refused 400 BadRequestException carrying crate::state::resource_not_usable_message for the PENDING state (tests/all/job_start_vocabulary_state.rs)
5. Read services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1807-1828 beside state.rs:2168-2174: the first says this family never reaches a usable state, the second says its build settles READY when the object store serves the file
