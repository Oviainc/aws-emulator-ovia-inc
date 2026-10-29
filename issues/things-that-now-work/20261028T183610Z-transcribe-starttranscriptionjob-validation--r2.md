Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: af40c6e1c3301a14f83f727add020b300628421a

# A job naming a custom LANGUAGE MODEL that is not COMPLETED is accepted — the status rule the state module delegates to the starts, which no start implements

_Filed: 2026-10-28T18:36:10Z_
_Operation: StartTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `af40c6e1c3301a14f83f727add020b300628421a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one place where a start checks the custom resources its settings name: `ComAmazonawsTranscribeTranscribeState::unheld_reference_error`. That helper has two halves — the resource must be HELD, and the resource must be in a state a job can apply it in (`unusable_reference_state` -> `resource_not_usable_message`). The state half is written for ONE family (the non-medical custom vocabulary) and falls through to `_ => None` for everything else.

For the custom LANGUAGE MODEL family the skip is deliberate, and its stated reason is an explicit DELEGATION, not a justification: `unusable_reference_state`'s own doc says the rule "is a rule about what a job START admits, which each start operation's own validation owns rather than this lookup". No start operation owns it. `MODEL_STATUS_COMPLETED` / `is_terminal_model_status` are consulted in exactly two handlers in the whole crate — `delete_language_model.rs:202` and `update_language_model.rs:225` — and in neither of the two starts that can name a model (`StartTranscriptionJob`, `StartCallAnalyticsJob`). So the rule that is single-homed nowhere is implemented nowhere.

Effect: a model created a millisecond earlier (and therefore still `IN_PROGRESS`), or one that settled `FAILED` because the object store said its training data could not be read, is accepted as a job's `ModelSettings.LanguageModelName` / `LanguageIdSettings[<code>].LanguageModelName` / Call Analytics `Settings.LanguageModelName`. The job is stored, reports the model setting back, and later reports `COMPLETED` as though a model that is not ready for use — or was never built at all — had been applied. The same crate refuses the identical shape of request for the sibling family: a start naming a `PENDING`/`FAILED` custom vocabulary is refused `BadRequestException` with `resource_not_usable_message`, through the very same helper call.

The model states the readiness condition the check is missing: `LanguageModel$ModelStatus` is "The status of the specified custom language model. When the status displays as COMPLETED the model is ready for use." The crate's own `LanguageModelRecord` doc repeats it verbatim and calls `COMPLETED` "the status that makes the model usable".

The fix is one match arm in `unusable_reference_state` (or the equivalent check in each start): `language_model_mut(region, name, now)` already exists and already materialises the clock-owed transition, `is_terminal_model_status` already exists for the non-terminal half, and `ResourceFamily::LanguageModel::kind()` already yields the wording `resource_not_usable_message` needs. Note for the fixer: three test sites create a language model and start a job naming it in the same breath, so they will need the "wait past the transition" step the vocabulary suite already uses (`crate::language_models::PAST_THE_TRANSITION`): `tests/all/transcription_job_request_rules.rs:686-694`, `tests/all/transcription_job_request_rules.rs:368` and `tests/all/call_analytics_job_start_limits.rs:95-134`.

## Evidence

The model sentence the handler contradicts — smithy_query(query="doc", shape="LanguageModel", member="ModelStatus"):
"The status of the specified custom language model. When the status displays as COMPLETED the model is ready for use."

The crate repeating it as its own invariant — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:2545-2548:
```rust
/// [`MODEL_STATUS_IN_PROGRESS`] at the create, then [`MODEL_STATUS_COMPLETED`] once
/// the clock makes the transition due, which is the status that makes the model usable
/// ("When the status displays as COMPLETED, your model is ready to use" — the model's
/// own doc for `ModelStatus`).
```

The DELEGATION nobody honours — state.rs:1803-1806 (doc of `unusable_reference_state`):
```rust
/// * a custom LANGUAGE model ([`LanguageModelRecord`]) is checked for existence
///   only: whether a job may name a model still training, or one whose training
///   failed, is a rule about what a job START admits, which each start operation's
///   own validation owns rather than this lookup;
```
and the code it justifies — state.rs:1814-1828:
```rust
    fn unusable_reference_state(
        &mut self,
        family: ResourceFamily,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<String> {
        match family {
            ResourceFamily::Vocabulary => {
                let record = self.vocabulary_mut(region, name, now)?;
                (record.state != VOCABULARY_STATE_READY).then(|| record.state.clone())
            }
            _ => None,
        }
    }
```

The rule the SAME helper applies for the sibling family — state.rs:1761-1765 and :1784-1786:
```rust
/// A resource that IS held must also be one a job can apply: a custom vocabulary
/// carries a `VocabularyState` precisely so that only a build that finished
/// successfully is usable ("If the state is READY, you can use the custom
/// vocabulary"), so one still being built or one whose build failed is refused with
/// [`resource_not_usable_message`]
```
```rust
            if let Some(state) = self.unusable_reference_state(*family, region, name, now) {
                return Some(resource_not_usable_message(family.kind(), name, &state));
            }
```

The call site that therefore gets only the existence half — src/ops/start_transcription_job.rs:489-500:
```rust
        let mut references = crate::state::custom_resource_references(
            settings.and_then(|settings| settings.vocabulary_name.as_deref()),
            settings.and_then(|settings| settings.vocabulary_filter_name.as_deref()),
            model_settings.and_then(|settings| settings.language_model_name.as_deref()),
        );
        crate::state::extend_language_id_references(
            &mut references,
            input.language_id_settings.as_ref(),
        );
        if let Some(message) = store.unheld_reference_error(&region, now, &references) {
            return Err(bad_request(&message));
        }
```
and identically at src/ops/start_call_analytics_job.rs:360-371 (`settings.and_then(|settings| settings.language_model_name.as_deref())`).

No start consults the model's status: grep for `MODEL_STATUS_COMPLETED|is_terminal_model_status` over `src/` matches only src/ops/delete_language_model.rs:202 (`Some(status) if !crate::state::is_terminal_model_status(&status) =>`), src/ops/update_language_model.rs:225 (`if !crate::state::is_terminal_model_status(&record.status) {`) and the definitions in state.rs:813-828.

Test coverage: nothing pins the current acceptance as a status rule. tests/all/job_start_vocabulary_state.rs:50-52 scopes its table to "Every member path of every start operation that names a custom vocabulary of the (non-medical) custom vocabulary family" and its header names only the vocabulary FILTER family as the counter-case (":21-23" — "that family models no processing state anywhere, so a held filter is applicable the moment it is created"); language models are absent from both. The one place a start naming a model is asserted 200 pins the FAMILY rule instead — tests/all/transcription_job_request_rules.rs:685-694:
```rust
    // Held in the right family too, the same name is accepted by each member.
    create_vocabulary_filter(&transcribe, name).await;
    create_language_model(&transcribe, name).await;
```

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateLanguageModel and body {"ModelName":"support-calls","LanguageCode":"en-US","BaseModelName":"WideBand","InputDataConfig":{"S3Uri":"s3://DOC-EXAMPLE-BUCKET/training/","DataAccessRoleArn":"arn:aws:iam::111122223333:role/Admin"}} — answered 200 with ModelStatus IN_PROGRESS
2. Immediately POST / with X-Amz-Target: Transcribe.StartTranscriptionJob and body {"TranscriptionJobName":"in-progress-model","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://example-media/audio.flac"},"ModelSettings":{"LanguageModelName":"support-calls"}} — answered 200 and the job is stored, although DescribeLanguageModel still reports IN_PROGRESS, which the model says is not "ready for use"
3. Create a model whose InputDataConfig.S3Uri names a location the object store says cannot be read, wait past its transition so DescribeLanguageModel reports ModelStatus FAILED, then repeat the start naming it — answered 200 again
4. Repeat step 2 through the other two member paths: StartTranscriptionJob with {"IdentifyLanguage":true,"LanguageOptions":["en-US","es-US"],"LanguageIdSettings":{"en-US":{"LanguageModelName":"support-calls"}}} and StartCallAnalyticsJob with Settings {"LanguageModelName":"support-calls"} — both answered 200
5. Compare the sibling family: CreateVocabulary {"VocabularyName":"terms","LanguageCode":"en-US","Phrases":["a"]} followed immediately by StartTranscriptionJob with Settings.VocabularyName "terms" is refused 400 BadRequestException carrying crate::state::resource_not_usable_message for the PENDING state (tests/all/job_start_vocabulary_state.rs)
6. Read services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1803-1828 beside the grep for is_terminal_model_status over src/ops: the first delegates the model-status rule to "each start operation's own validation", the second shows no start operation has one
