Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-language-model-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-language-model-lifecycle transcribe-medical-transcription-job-workflow transcribe-medical-vocabulary-lifecycle transcribe-vocabulary-lifecycle
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|DescribeLanguageModel|4xx->4xx|$.Message
Issue-Key: transcribe|StartMedicalTranscriptionJob|4xx->4xx|$.Message
Issue-Key: transcribe|GetMedicalVocabulary|4xx->4xx|$.Message
Issue-Key: transcribe|CreateVocabulary|4xx->4xx|$.Message
Issue-Key: transcribe|DeleteVocabulary|2xx->4xx|$
Issue-Key: transcribe|DeleteVocabulary|2xx->4xx|status
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Transcribe refusal wording is reworded on four operations and DeleteVocabulary refuses a delete AWS accepts

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLanguageModel

## Description
Five remaining error-path divergences across the language-model, medical-transcription, medical-vocabulary and vocabulary families. Four of them return the right error code and status with a message the emulator wrote itself (and in two cases a message describing a DIFFERENT failure than the one AWS reports); the fifth refuses a DeleteVocabulary that AWS answers with 200. 5 causes.

Smithy for all of them: BadRequestException (400, Message -> FailureReason), doc "Your request didn't pass one or more validation tests. This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS). See the exception message field for more information." Because that one code covers not-found, non-terminal-state and validation failures alike, the message is the only discriminator a client has, and the per-class text is recorded behavior only.

Cause 1 of 5: DescribeLanguageModel on a model name that does not exist.
AWS: BadRequestException, 400, fixed text "The specified custom language model couldn't be found. Check the language model name and try your request again." — note "specified custom language model"; the ModelName is not interpolated.
Emulator: BadRequestException, 400, "The requested language model couldn't be found. Check the language model name and try your request again." — reworded first clause.
Why: a client keyed on AWS's text cannot identify a missing custom language model and cannot separate it from the operation's other 400s.
Fix: the not-found message constant on DescribeLanguageModel's lookup path in the transcribe handler.
Traces: transcribe-language-model-lifecycle#task#1

Cause 2 of 5: StartMedicalTranscriptionJob whose Media/output bucket does not exist — the emulator reports the wrong failure.
AWS: BadRequestException, 400, fixed text "The specified S3 bucket doesn't exist. Make sure the bucket exists and try your request again." — AWS distinguishes a missing BUCKET from a missing object, and interpolates neither the bucket name nor the URI.
Emulator: BadRequestException, 400, "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." — the missing-object message, which tells the caller the object is absent when the bucket itself is.
Why: the diagnosis is actively misleading: a caller (or an operator) is sent to check a key inside a bucket that was never created, and automation that branches missing-bucket vs missing-object on the message takes the wrong path.
Fix: StartMedicalTranscriptionJob's media/output validation in the transcribe handler must resolve bucket existence separately from object existence and emit the bucket-missing fixed text for the former.
Traces: transcribe-medical-transcription-job-workflow#task#2

Cause 3 of 5: GetMedicalVocabulary on a vocabulary name that does not exist.
AWS: BadRequestException, 400, fixed text "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." — AWS uses the PLAIN vocabulary wording for the medical family too; the word "medical" does not appear and the name is not interpolated.
Emulator: BadRequestException, 400, "The requested medical vocabulary couldn't be found. Check the medical vocabulary name and try your request again." — "medical" inserted in both clauses.
Why: a client that matches the documented/recorded vocabulary-not-found text (and shares that matcher with the non-medical family, as AWS's wording permits) fails to recognise the refusal.
Fix: the not-found message constant used by the medical-vocabulary read path in the transcribe handler — reuse the plain vocabulary text.
Traces: transcribe-medical-vocabulary-lifecycle#task#1

Cause 4 of 5: CreateVocabulary with neither Phrases nor VocabularyFileUri.
AWS: BadRequestException, 400, fixed text "Either Phrases or VocabularyFileUri field should be provided." — a short, entirely fixed sentence naming the two members; no request values.
Emulator: BadRequestException, 400, "Your request must include either Phrases or VocabularyFileUri. Add the terms of your custom vocabulary and try your request again.".
Why: correct class, unrecognisable text; the either/or rule is service-side (Smithy marks both Phrases and VocabularyFileUri optional), so the message is all a client has.
Fix: CreateVocabulary's either/or input validation in the transcribe handler.
Traces: transcribe-vocabulary-lifecycle#task#2
Note for the fixer: the open behavioral issue 20261029T184620Z-transcribe-rejection-conflict-error-messages-are-emulator-authored records this same expected text as its Cause 2 from the behavioral corpus. It carries no wire replay key, so this wire step is filed here; fixing the constant should close both, but this step must be re-replayed to confirm.

Cause 5 of 5: DeleteVocabulary is refused where AWS answers 200.
AWS: HTTP 200 with an empty body ({}) for this DeleteVocabulary (the operation models Unit output), i.e. the delete is accepted.
Emulator: HTTP 400 BadRequestException with "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." — it has no vocabulary under that name to delete.
Why: a successful teardown becomes a hard failure, so cleanup aborts and the name is never released; a client cannot tell a real misuse from this. Hypothesis (not stated by the model or the recording): the emulator's earlier handling in this lifecycle left no vocabulary under the name the recording deletes — either it refused a create AWS accepted or it dropped the record — so the fix must establish which, rather than only relaxing the delete guard.
Fix: DeleteVocabulary's lookup/guard path in the transcribe handler and the vocabulary state the preceding steps of the lifecycle leave behind.
Traces: transcribe-vocabulary-lifecycle#task#5

## Evidence
Cause 1 (transcribe-language-model-lifecycle#task#1, DescribeLanguageModel, 400 -> 400): $.Message value_mismatch — expected "The specified custom language model couldn't be found. Check the language model name and try your request again.", got "The requested language model couldn't be found. Check the language model name and try your request again.". Fixed text: the whole message; variable parts: none.
Cause 2 (transcribe-medical-transcription-job-workflow#task#2, StartMedicalTranscriptionJob, 400 -> 400): $.Message value_mismatch — expected "The specified S3 bucket doesn't exist. Make sure the bucket exists and try your request again.", got "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again.". Both texts are entirely fixed; they denote different failures.
Cause 3 (transcribe-medical-vocabulary-lifecycle#task#1, GetMedicalVocabulary, 400 -> 400): $.Message value_mismatch — expected "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again.", got "The requested medical vocabulary couldn't be found. Check the medical vocabulary name and try your request again.".
Cause 4 (transcribe-vocabulary-lifecycle#task#2, CreateVocabulary, 400 -> 400): $.Message value_mismatch — expected 'Either Phrases or VocabularyFileUri field should be provided.', got 'Your request must include either Phrases or VocabularyFileUri. Add the terms of your custom vocabulary and try your request again.'.
Cause 5 (transcribe-vocabulary-lifecycle#task#5, DeleteVocabulary): status expected 200, got 400; body expected {}, got {'__type': 'BadRequestException', 'Message': "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again."}.
In causes 1-4 the error code (BadRequestException) and HTTP status (400) matched; only the message diverged.

## Steps to Reproduce
1. DescribeLanguageModel(ModelName=<a name never created>) and read Message of the 400.
2. StartMedicalTranscriptionJob(MedicalTranscriptionJobName=<new>, LanguageCode='en-US', Specialty='PRIMARYCARE', Type='CONVERSATION', Media={MediaFileUri:'s3://<bucket-that-was-never-created>/<key>'}, OutputBucketName=<that same nonexistent bucket>) and read Message of the 400 — expect the bucket-missing text, observe the object-missing text.
3. GetMedicalVocabulary(VocabularyName=<a name never created>) and read Message of the 400.
4. CreateVocabulary(VocabularyName=<new>, LanguageCode='en-US') with no Phrases and no VocabularyFileUri; read Message of the 400.
5. Run the vocabulary lifecycle through to its DeleteVocabulary step and compare: AWS answers HTTP 200 with an empty body, the emulator answers 400 with the vocabulary-not-found payload.
Replay: transcribe-language-model-lifecycle#task#1, transcribe-medical-transcription-job-workflow#task#2, transcribe-medical-vocabulary-lifecycle#task#1, transcribe-vocabulary-lifecycle#task#2 and transcribe-vocabulary-lifecycle#task#5.
