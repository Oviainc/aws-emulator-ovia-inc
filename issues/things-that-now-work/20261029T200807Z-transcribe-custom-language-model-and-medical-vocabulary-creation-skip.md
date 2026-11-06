Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-language-model-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-language-model-lifecycle transcribe-medical-vocabulary-lifecycle transcribe-not-found-error-paths
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|CreateLanguageModel|4xx->2xx|$.Message
Issue-Key: transcribe|CreateLanguageModel|4xx->2xx|$.__type
Issue-Key: transcribe|CreateLanguageModel|4xx->2xx|status
Issue-Key: transcribe|ListLanguageModels|2xx->2xx|$.Models[*]
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->2xx|$.Message
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->2xx|$.__type
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->2xx|status
Issue-Key: transcribe|ListMedicalVocabularies|2xx->2xx|$.Vocabularies[*]
Issue-Key: transcribe|UpdateMedicalVocabulary|4xx->4xx|$.Message
Issue-Key: transcribe|DeleteMedicalVocabulary|2xx->4xx|$
Issue-Key: transcribe|DeleteMedicalVocabulary|2xx->4xx|status
Issue-Key: transcribe|GetMedicalVocabulary|4xx->2xx|$.Message
Issue-Key: transcribe|GetMedicalVocabulary|4xx->2xx|$.__type
Issue-Key: transcribe|GetMedicalVocabulary|4xx->2xx|status
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Custom language model and medical vocabulary creation skip S3 input validation, so the whole lifecycle diverges

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLanguageModel

## Description
Two create operations that take an S3 input location — CreateLanguageModel (InputDataConfig.S3Uri) and CreateMedicalVocabulary (VocabularyFileUri) — accept a syntactically valid URI that resolves to nothing and create the resource, where AWS refuses the call synchronously with BadRequestException/400 and creates nothing. Because the resource then exists only in the emulator, every later step of each lifecycle diverges too: the lists report a resource AWS does not have, and the subsequent update/delete/get of the medical vocabulary answer about an object that should never have existed. 7 causes: 2 root over-permissive validations and 5 downstream consequences a fixer must confirm individually, because each one is a separate step verdict.

Smithy: CreateLanguageModel requires InputDataConfig (S3Uri target Uri, @length 1..2000, @pattern ^(s3://|http(s*)://).+$) and models BadRequestException (400); CreateMedicalVocabulary requires VocabularyFileUri with the same Uri constraints and also models BadRequestException (400). The model constrains the URI only syntactically, so reachability validation and the message texts are recorded behavior only.

Cause 1 of 7: CreateLanguageModel does not validate that InputDataConfig.S3Uri is readable.
AWS: BadRequestException, HTTP 400, with the fixed text "The S3 URI that you provided can't be accessed. Make sure that you have read permission and try your request again." (no request values interpolated), and NO language model is created.
Emulator: HTTP 200, no __type and no Message — the model is created and reported with ModelStatus IN_PROGRESS.
Why: a negative class becomes a success. A client relying on the synchronous refusal to catch a mistyped bucket/prefix or a role that cannot read it instead gets an accepted model that will never train, and its error-handling path is never exercised.
Fix: CreateLanguageModel's input validation in the transcribe handler — resolve InputDataConfig.S3Uri against S3 state before creating anything and refuse with the fixed message above.
Traces: transcribe-language-model-lifecycle#task#2

Cause 2 of 7: ListLanguageModels reports the model AWS refused to create.
AWS: 200 with an empty Models list (the create was rejected, so nothing is listed).
Emulator: 200 with Models[0] present describing the model it accepted in cause 1 (ModelName, LanguageCode, BaseModelName, ModelStatus IN_PROGRESS, UpgradeAvailability, InputDataConfig echoing the unreachable S3Uri).
Why: the account's inventory contains a resource AWS would not have; anything that enumerates models to decide what exists is wrong.
Fix: follows from cause 1 (nothing to list once the create is refused); verify this step separately.
Traces: transcribe-language-model-lifecycle#task#3

Cause 3 of 7: CreateMedicalVocabulary does not validate that VocabularyFileUri points at an existing S3 object.
AWS: BadRequestException, HTTP 400, with the fixed text "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." (no request values interpolated), and NO medical vocabulary is created.
Emulator: HTTP 200 — the vocabulary is created in state PENDING.
Why: same over-permissive negative class as cause 1, on the medical vocabulary family.
Fix: CreateMedicalVocabulary's input validation in the transcribe handler — resolve VocabularyFileUri against S3 state before creating the vocabulary.
Traces: transcribe-medical-vocabulary-lifecycle#task#2

Cause 4 of 7: ListMedicalVocabularies reports the vocabulary AWS refused to create.
AWS: 200 with an empty Vocabularies list.
Emulator: 200 with Vocabularies[0] present (VocabularyName, LanguageCode, LastModifiedTime, VocabularyState PENDING).
Why: as cause 2, for medical vocabularies.
Fix: follows from cause 3; verify separately.
Traces: transcribe-medical-vocabulary-lifecycle#task#3

Cause 5 of 7: UpdateMedicalVocabulary on that name answers a state guard instead of not-found.
AWS: BadRequestException, 400, with the fixed text "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." — the name does not exist, and AWS words the medical-vocabulary not-found with the plain "vocabulary" wording (no "medical" in it) and does not interpolate the name.
Emulator: BadRequestException, 400, with "The custom medical vocabulary <VocabularyName> is in the <VocabularyState> state and can't be updated. Wait until it is no longer being processed and try your request again." — an emulator-authored template filled with the request's vocabulary name and the stored state (observed PENDING). Both the class of refusal and the wording are wrong, and AWS's text here is entirely fixed, so interpolating the name makes a stable match impossible.
Why: the caller is told to wait for a resource that does not exist on AWS at all, so it polls forever instead of correcting its request.
Fix: follows from cause 3 for the class; independently, the medical-vocabulary not-found message constant must use the plain-vocabulary fixed text with no interpolation.
Traces: transcribe-medical-vocabulary-lifecycle#task#4 (the same cluster also covers transcribe-not-found-error-paths#task#4)

Cause 6 of 7: DeleteMedicalVocabulary is refused where AWS succeeds.
AWS: HTTP 200 with an empty body ({}) — DeleteMedicalVocabulary models Unit output, and in the recording the delete of this name succeeds rather than refusing.
Emulator: HTTP 400 BadRequestException with "The custom medical vocabulary <VocabularyName> is in the <PENDING> state and can't be deleted. Wait until it is no longer being processed and try your request again." — again name and state interpolated into emulator-authored text.
Why: a successful teardown becomes a hard failure, so cleanup logic aborts and the name is never released. Even once cause 3 is fixed, the delete path must answer 200/empty here rather than a PENDING-state guard.
Fix: DeleteMedicalVocabulary's state guard in the transcribe handler, together with the state cause 3 leaves behind.
Traces: transcribe-medical-vocabulary-lifecycle#task#5

Cause 7 of 7: GetMedicalVocabulary after the delete returns the vocabulary instead of refusing.
AWS: BadRequestException, 400, with the fixed text "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again.".
Emulator: HTTP 200 with the vocabulary's properties, no __type and no Message.
Why: the resource outlives both the refused create and the delete, so a client's existence check answers yes for something AWS reports as absent.
Fix: follows from causes 3 and 6; verify separately.
Traces: transcribe-medical-vocabulary-lifecycle#task#6

Related but not covering: the open behavioral issue 20261029T184620Z-transcribe-createvocabulary-accepts-a-vocabularyfileuri-pointing-at-a-nonexistent reports the same over-permissive URI handling for CreateVocabulary (the non-medical operation) only; CreateLanguageModel and CreateMedicalVocabulary are not among its causes and it carries no wire replay keys.

## Evidence
Cause 1 (transcribe-language-model-lifecycle#task#2, CreateLanguageModel): status expected 400, got 200; $.__type missing (expected 'BadRequestException'); $.Message missing (expected "The S3 URI that you provided can't be accessed. Make sure that you have read permission and try your request again." — entirely fixed text).
Cause 2 (transcribe-language-model-lifecycle#task#3, ListLanguageModels): 200 -> 200, $.Models[0] extra (expected None) — a model entry with ModelStatus 'IN_PROGRESS' and InputDataConfig.S3Uri pointing at the unreachable training prefix. The generated model name, bucket name and timestamps are per-run values.
Cause 3 (transcribe-medical-vocabulary-lifecycle#task#2, CreateMedicalVocabulary): status expected 400, got 200; $.__type missing (expected 'BadRequestException'); $.Message missing (expected "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." — entirely fixed text).
Cause 4 (transcribe-medical-vocabulary-lifecycle#task#3, ListMedicalVocabularies): 200 -> 200, $.Vocabularies[0] extra (expected None) — an entry with VocabularyState 'PENDING'.
Cause 5 (transcribe-medical-vocabulary-lifecycle#task#4, UpdateMedicalVocabulary, 400 -> 400): $.Message value_mismatch — expected "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." (all fixed text), got "The custom medical vocabulary <name> is in the PENDING state and can't be updated. Wait until it is no longer being processed and try your request again." where <name> is the request's VocabularyName and PENDING is the emulator's stored VocabularyState — request/state values filling an emulator-invented template.
Cause 6 (transcribe-medical-vocabulary-lifecycle#task#5, DeleteMedicalVocabulary): status expected 200, got 400; body expected {} , got {'__type': 'BadRequestException', 'Message': "The custom medical vocabulary <name> is in the PENDING state and can't be deleted. Wait until it is no longer being processed and try your request again."}.
Cause 7 (transcribe-medical-vocabulary-lifecycle#task#6, GetMedicalVocabulary): status expected 400, got 200; $.__type missing (expected 'BadRequestException'); $.Message missing (expected the fixed not-found text above).

## Steps to Reproduce
Language model leg:
1. CreateLanguageModel(ModelName=<new>, LanguageCode='en-US', BaseModelName='NarrowBand', InputDataConfig={S3Uri:'s3://<bucket>/training-data/', DataAccessRoleArn:'arn:aws:iam::000000000000:role/<role>'}) where that S3 prefix holds no readable object. Expect BadRequestException/400 with the S3-URI message and no model; observe HTTP 200 with ModelStatus IN_PROGRESS.
2. ListLanguageModels() — expect no Models entry; observe the model from step 1.
Medical vocabulary leg:
3. CreateMedicalVocabulary(VocabularyName=<new>, LanguageCode='en-US', VocabularyFileUri='s3://<bucket>/<key-that-does-not-exist>'). Expect BadRequestException/400 with the S3-object message and no vocabulary; observe HTTP 200, VocabularyState PENDING.
4. ListMedicalVocabularies() — expect no Vocabularies entry; observe one.
5. UpdateMedicalVocabulary(VocabularyName=<same>, LanguageCode='en-US', VocabularyFileUri=<same>) — expect the fixed vocabulary-not-found message; observe the PENDING-state template.
6. DeleteMedicalVocabulary(VocabularyName=<same>) — expect HTTP 200 with an empty body; observe 400 with the PENDING-state template.
7. GetMedicalVocabulary(VocabularyName=<same>) — expect BadRequestException/400 not-found; observe HTTP 200 with the vocabulary.
Replay: transcribe-language-model-lifecycle task#2, task#3 and transcribe-medical-vocabulary-lifecycle task#2 through task#6.
