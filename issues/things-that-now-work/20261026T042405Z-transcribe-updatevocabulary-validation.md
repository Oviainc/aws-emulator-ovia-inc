Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# UpdateVocabulary overwrites a PENDING vocabulary instead of refusing the documented non-terminal state

_Filed: 2026-10-26T04:24:05Z_
_Operation: UpdateVocabulary | Class: validation_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateVocabulary`'s own modelled documentation states two preconditions: the vocabulary must be in a terminal state, and the request must name exactly one term source. The handler implements the second one in full (the `content_source` Neither/Both refusals) but never looks at the stored `VocabularyState` at all: it takes the record with a plain `store.vocabularies.get_mut(...)` and immediately overwrites `language_code`, `phrases`, `file_uri`, `state`, `failure_reason`, `transition_due` and `last_modified_time`.

So an update issued while the vocabulary is still `PENDING` — i.e. inside the `VOCABULARY_TRANSITION_DELAY` window after a create or a previous update — is answered 200 and silently destroys the in-flight build's terms, where the operation documents a refusal. The sibling `DeleteVocabulary` in the same crate refuses exactly that state for exactly that record, and it reaches the record through the crate's own clock-materialising lookup `store.vocabulary_mut(&region, name, now)`, which this handler also skips, so it does not even see the state the clock already owed the record.

## Evidence

Smithy model — `smithy_query(query="doc", shape="com.amazonaws.transcribe#UpdateVocabulary")`:
  "Updates an existing custom vocabulary with new values. This operation overwrites all existing information with your new values; you cannot append new terms onto an existing custom vocabulary."
  "**Your custom vocabulary must be in a terminal state (`READY` or `FAILED`) before you can update it.** You must include either `Phrases` or `VocabularyFileUri` in your request."
The same sentence is carried in the handler's own header: services/service-com-amazonaws-transcribe-transcribe/src/ops/update_vocabulary.rs:47 — "//! custom vocabulary. Your custom vocabulary must be in a terminal state (READY or FAILED)".

Handler, no state check anywhere between lookup and overwrite — services/service-com-amazonaws-transcribe-transcribe/src/ops/update_vocabulary.rs:255-278:
```
        let Some(record) = store
            .vocabularies
            .get_mut(&region, &input.vocabulary_name)
        else {
...
        record.language_code = input.language_code;
        record.phrases = phrases;
        record.file_uri = file_uri;
        record.state = crate::state::VOCABULARY_STATE_PENDING.to_string();
```

Sibling in the SAME crate that does enforce it, and that uses the crate's clock-materialising lookup — services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs:192-216:
```
        let stored_state = store
            .vocabulary_mut(&region, &input.vocabulary_name, now)
            .map(|record| record.state.clone());
        match stored_state {
            Some(vocabulary_state)
                if vocabulary_state == crate::state::VOCABULARY_STATE_PENDING =>
            {
                return Err(DeleteVocabularyErrorModeled::BadRequestException(
```
and the helper it calls — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1356-1367: "/// The custom vocabulary named `name`, with every clock-owed transition of it /// already materialised".

Test coverage: the crate's update tests deliberately sleep past the transition first, so none of them pins the PENDING-accepted behaviour. services/service-com-amazonaws-transcribe-transcribe/tests/all/vocabulary_update_term_source.rs:257 even names the rule it is establishing — "// The precondition the operation publishes: the record is in a terminal state." — and services/service-com-amazonaws-transcribe-transcribe/tests/all/vocabularies.rs:423-431 asserts `"READY"` with "the vocabulary must be READY before the update, or the reset proves nothing" before updating (and sleeps again at :481 between rows). No existing test calls UpdateVocabulary on a PENDING record.

## Steps to Reproduce

1. CreateVocabulary with VocabularyName="pend", LanguageCode="en-US", Phrases=["Aardvark"] — answers VocabularyState=PENDING with transition_due = now + VOCABULARY_TRANSITION_DELAY (500 ms, state.rs:462).
2. Immediately (well inside 500 ms, i.e. without sleeping) call UpdateVocabulary with VocabularyName="pend", LanguageCode="fr-FR", Phrases=["Zebre"].
3. Observed: HTTP 200 with VocabularyState=PENDING and the new language/terms stored — the in-flight build is discarded.
4. Expected per the operation's own documentation sentence: the update is refused because the vocabulary is not in a terminal state (READY or FAILED), the same way DeleteVocabulary refuses a PENDING vocabulary at delete_vocabulary.rs:202-216.
5. Contrast: GetVocabulary for "pend" right after the create reports PENDING, confirming the non-terminal state was observable to the update.
