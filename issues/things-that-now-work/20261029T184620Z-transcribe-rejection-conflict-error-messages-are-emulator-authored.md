Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.concurrent-create-vocabulary-same-name-one-wins transcribe.create-vocabulary-without-phrases-or-fileuri-rejected transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected transcribe.start-transcription-job-without-language-params-rejected transcribe.delete-vocabulary-while-pending-rejected transcribe.delete-transcription-job-while-in-progress-rejected
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.concurrent-create-vocabulary-same-name-one-wins|run#10|AssertionError|assert all(m == _CONFLICT_MESSAGE for m in conflicts), (
Issue-Key: transcribe.create-vocabulary-without-phrases-or-fileuri-rejected|run#6|AssertionError|assert message == expected, f"expected missing-source message {expected!r}, got {message!r}"
Issue-Key: transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected|run#6|AssertionError|assert message == expected, f"expected missing-source message {expected!r}, got {message!r}"
Issue-Key: transcribe.start-transcription-job-without-language-params-rejected|run#10|AssertionError|assert message == expected, f"expected missing-language message {expected!r}, got {message!r}"
Issue-Key: transcribe.delete-vocabulary-while-pending-rejected|run#9|AssertionError|assert message == expected, f"expected pending-delete message {expected!r}, got {message!r}"
Issue-Key: transcribe.delete-transcription-job-while-in-progress-rejected|run#14|AssertionError|assert message == expected, f"expected active-job delete message {expected!r}, got {message!r}"
Issue-Commit: 96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9

# Transcribe rejection/conflict error messages are emulator-authored prose, not AWS's wording (6 operations)

> _Note: this issue was identified against the code base at commit `96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVocabulary

## Description
Six distinct Transcribe refusal paths return the CORRECT error code and HTTP status but a message the emulator wrote itself instead of the message AWS returns for that error class. Smithy models only the carrier (BadRequestException.Message -> FailureReason, 400, doc: "Your request didn't pass one or more validation tests. This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS). See the exception message field for more information."; ConflictException.Message -> String, 409, doc: "A resource already exists with this name..."), so the exact text per class is recorded behavior only. The message is part of the contract: clients and test suites branch on it, and in Transcribe's case the code alone (BadRequestException) is used for not-found, non-terminal-state AND missing-field, so the message is the only thing that distinguishes them.

All six have the same shape: right code, wrong text. Five of the six emulator messages also interpolate the resource name or the state into text whose AWS counterpart is entirely FIXED, i.e. the emulator invents variable parts AWS does not have.

Cause 1 of 6 — CreateVocabulary name conflict (concurrent racers).
AWS: recorded behavior only. The losing racers of N concurrent CreateVocabulary calls on one name get ConflictException (409) with the entirely fixed text "The requested job name already exists. Use a different job name." (AWS reuses the job-name wording for a vocabulary-name conflict; no part of it comes from the request).
Emulator: ConflictException with "The requested vocabulary name already exists. Use a different vocabulary name." — 7 of 8 racers, exactly one winner (the one-winner semantics are correct; only the text diverges).
Why: a client matching the recorded conflict text does not recognise the refusal.
NOTE for the fixer: the sibling check transcribe.create-vocabulary-duplicate-name-conflict currently PASSES and asserts the vocabulary-name wording for the SEQUENTIAL duplicate-name path, so per the recordings the two paths carry different fixed text. Satisfy the concurrent path without regressing the sequential one; do not globally rename one into the other.
Fix: CreateVocabulary's conflict path in the transcribe handler (the message constant used when an existing vocabulary name is detected under concurrent insert).
Traces: transcribe.concurrent-create-vocabulary-same-name-one-wins

Cause 2 of 6 — CreateVocabulary with neither Phrases nor VocabularyFileUri.
AWS: recorded behavior only; Smithy makes both Phrases and VocabularyFileUri optional, so the either/or rule is service-side validation. BadRequestException (400) with the entirely fixed text "Either Phrases or VocabularyFileUri field should be provided." (no variable parts).
Emulator: BadRequestException with "Your request must include either Phrases or VocabularyFileUri. Add the terms of your custom vocabulary and try your request again."
Why: the refusal is correct but unrecognisable to a client keyed on AWS's text.
Fix: CreateVocabulary input validation in the transcribe handler.
Traces: transcribe.create-vocabulary-without-phrases-or-fileuri-rejected

Cause 3 of 6 — CreateVocabularyFilter with neither Words nor VocabularyFilterFileUri.
AWS: recorded behavior only; Smithy makes Words (@length min 1) and VocabularyFilterFileUri both optional. BadRequestException (400) with the entirely fixed text "You need to provide either the Words field or the VocabularyFilterFileUri field. Provide one of the fields and try your request again."
Emulator: BadRequestException with "Your request must include either Words or VocabularyFilterFileUri. Add the words of your custom vocabulary filter and try your request again."
Why: same — correct class, wrong text.
Fix: CreateVocabularyFilter input validation in the transcribe handler.
Traces: transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected

Cause 4 of 6 — StartTranscriptionJob with none of LanguageCode / IdentifyLanguage / IdentifyMultipleLanguages.
AWS: recorded behavior only; Smithy leaves LanguageCode, IdentifyLanguage and IdentifyMultipleLanguages all optional on StartTranscriptionJobRequest. BadRequestException (400) with the entirely fixed text "The language code is missing. Either add a language code or turn on language identification."
Emulator: BadRequestException with "Your request must include exactly one of LanguageCode, IdentifyLanguage, or IdentifyMultipleLanguages. Correct your request and try again."
Why: same.
Fix: StartTranscriptionJob input validation in the transcribe handler.
Traces: transcribe.start-transcription-job-without-language-params-rejected

Cause 5 of 6 — DeleteVocabulary while the vocabulary is still PENDING.
AWS: recorded behavior only (consistent with the BadRequestException doc's "non-terminal state" clause). BadRequestException (400) with the entirely fixed text "The vocabulary that you\u2019re trying to delete isn't in the READY or FAILED state. Try your request again when the vocabulary state is READY or FAILED." — note the apostrophe is U+2019 (RIGHT SINGLE QUOTATION MARK), not ASCII ', and the vocabulary name does NOT appear in it.
Emulator: BadRequestException with "The custom vocabulary <VocabularyName> is in the <VocabularyState> state and can't be deleted. Wait until it is no longer being processed and try your request again." (observed with the request's vocabulary name and state PENDING filled in — those are request/state values filling an emulator-invented template, not AWS's message).
Why: besides the wrong text, the emulator leaks the resource name into a message AWS keeps fixed, so no stable match is possible.
Fix: DeleteVocabulary's state-guard path in the transcribe handler.
Traces: transcribe.delete-vocabulary-while-pending-rejected

Cause 6 of 6 — DeleteTranscriptionJob while the job is IN_PROGRESS.
AWS: recorded behavior only (same "non-terminal state (such as IN PROGRESS)" clause in the BadRequestException doc). BadRequestException (400) with the entirely fixed text "The job couldn't be deleted. Make sure it exists and isn't pending." — no variable parts.
Emulator: BadRequestException with "The transcription job <TranscriptionJobName> is in the <TranscriptionJobStatus> state and can't be deleted. Wait until it is no longer being processed and try your request again." (observed with the request's job name and status IN_PROGRESS interpolated).
Why: same as cause 5.
Fix: DeleteTranscriptionJob's state-guard path in the transcribe handler.
Traces: transcribe.delete-transcription-job-while-in-progress-rejected

## Evidence
Cause 1: expected ConflictException message 'The requested job name already exists. Use a different job name.'; got, for 7 of 8 racers, 'The requested vocabulary name already exists. Use a different vocabulary name.'
Cause 2: expected 'Either Phrases or VocabularyFileUri field should be provided.'; got 'Your request must include either Phrases or VocabularyFileUri. Add the terms of your custom vocabulary and try your request again.'
Cause 3: expected 'You need to provide either the Words field or the VocabularyFilterFileUri field. Provide one of the fields and try your request again.'; got 'Your request must include either Words or VocabularyFilterFileUri. Add the words of your custom vocabulary filter and try your request again.'
Cause 4: expected 'The language code is missing. Either add a language code or turn on language identification.'; got 'Your request must include exactly one of LanguageCode, IdentifyLanguage, or IdentifyMultipleLanguages. Correct your request and try again.'
Cause 5: expected "The vocabulary that you\u2019re trying to delete isn't in the READY or FAILED state. Try your request again when the vocabulary state is READY or FAILED."; got "The custom vocabulary <name> is in the PENDING state and can't be deleted. Wait until it is no longer being processed and try your request again."
Cause 6: expected "The job couldn't be deleted. Make sure it exists and isn't pending."; got "The transcription job <name> is in the IN_PROGRESS state and can't be deleted. Wait until it is no longer being processed and try your request again."
In every case the error CODE (ConflictException / BadRequestException) and the HTTP status matched; only the message text diverged. The not-found messages these same checks assert ('The requested vocabulary couldn't be found. Check the vocabulary name and try your request again.', 'The requested job couldn't be found. Check the job name and try your request again.', 'The requested vocabulary filter couldn't be found. Check the vocabulary filter name and try your request again.') were accepted, so the divergence is confined to the six paths above.

## Steps to Reproduce
1. CreateVocabulary(VocabularyName=N, LanguageCode=en-US, Phrases=[...]) from 8 threads concurrently with the SAME N; read the Message of each ConflictException loser.
2. CreateVocabulary(VocabularyName=N2, LanguageCode=en-US) with no Phrases and no VocabularyFileUri; read the BadRequestException Message.
3. CreateVocabularyFilter(VocabularyFilterName=F, LanguageCode=en-US) with no Words and no VocabularyFilterFileUri; read the Message.
4. StartTranscriptionJob(TranscriptionJobName=J, Media={MediaFileUri:s3://b/k}) with no LanguageCode/IdentifyLanguage/IdentifyMultipleLanguages; read the Message.
5. CreateVocabulary a vocabulary, then immediately DeleteVocabulary while its VocabularyState is PENDING; read the Message.
6. StartTranscriptionJob, then DeleteTranscriptionJob while TranscriptionJobStatus is IN_PROGRESS; read the Message.
Replay: the six check ids in trace_ids.
