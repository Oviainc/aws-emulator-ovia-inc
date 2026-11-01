Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.vocabulary-transitions-to-failed-on-unreachable-file transcribe.transcription-job-identify-language-reflects-detected-code
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.vocabulary-transitions-to-failed-on-unreachable-file|_client_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: transcribe.transcription-job-identify-language-reflects-detected-code|run#18|AssertionError|assert fr_job["LanguageCode"] == "fr-FR", f"expected fr-FR to be identified, got {fr_job.get('LanguageCode')}"
Issue-Commit: 96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9

# CreateVocabulary accepts a VocabularyFileUri pointing at a nonexistent bucket, and IdentifyLanguage always answers en-US

> _Note: this issue was identified against the code base at commit `96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVocabulary

## Description
Two independent fidelity defects in the transcribe crate: one over-permissive validation path, one hardcoded data-plane result.

Cause 1 of 2 — CreateVocabulary does not validate that VocabularyFileUri points at a reachable S3 object.
AWS: recorded behavior only. Smithy constrains VocabularyFileUri only syntactically (target Uri, @length 1..2000, @pattern ^(s3://|http(s*)://).+$) and CreateVocabulary models BadRequestException (400). The recording shows AWS resolving the URI as part of request validation: a syntactically valid s3:// URI naming a bucket that was never created is refused SYNCHRONOUSLY with BadRequestException (400) and the fixed text "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." No vocabulary is created — a following GetVocabulary on that name reports BadRequestException "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." The vocabulary is NOT accepted and then transitioned to FAILED asynchronously.
Emulator: the call SUCCEEDS with HTTP 200, returning VocabularyName, LanguageCode=en-US, VocabularyState=PENDING and LastModifiedTime; a vocabulary is created under that name.
Why: the emulator is over-permissive on a negative class. A client that relies on the synchronous refusal to surface a mistyped or unauthorized bucket instead sees a created resource and must discover the problem later (or never, if the emulator never drives it to FAILED). Error-handling code around CreateVocabulary is never exercised, and the account accumulates vocabularies AWS would have refused to create.
Fix: CreateVocabulary's input validation in the transcribe handler — resolve VocabularyFileUri against S3 state before creating the vocabulary, and on an unresolvable object return BadRequestException with the fixed message above and create nothing.
Traces: transcribe.vocabulary-transitions-to-failed-on-unreachable-file

Cause 2 of 2 — IdentifyLanguage reports a hardcoded en-US rather than the detected language.
AWS: TranscriptionJob.IdentifyLanguage is documented as "Indicates whether automatic language identification was enabled (TRUE) for the specified transcription job", and TranscriptionJob.LanguageCode as "The language code used to create your transcription job" with IdentifiedLanguageScore "The confidence score associated with the language identified in your media file". With IdentifyLanguage=True and LanguageOptions=[en-US, es-ES, fr-FR], the completed job's LanguageCode must be the language actually spoken in the media — English speech yields en-US, French speech yields fr-FR — each with IdentifiedLanguageScore > 0.5.
Emulator: both jobs complete reporting LanguageCode=en-US. The English leg is right only by coincidence; the French-audio job (French speech synthesized via Polly, voice Lea) also reports en-US, i.e. the identified language is a constant default, not a function of the media. LanguageOptions is ignored as a signal.
Why: a client that branches on the identified language (routing, post-processing, per-language vocabularies, storing the detected code) is silently given the wrong answer for every non-English input, with no error to indicate it. Language identification appears to work while being a fixed answer.
Fix: the language-identification path in the transcribe crate's transcription-job processing (where LanguageCode / LanguageCodes / IdentifiedLanguageScore are set for a job started with IdentifyLanguage=True) — derive the code from the media within LanguageOptions instead of defaulting to en-US.
Traces: transcribe.transcription-job-identify-language-reflects-detected-code

## Evidence
Cause 1: expected BadRequestException (400) with message "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." for CreateVocabulary(VocabularyName=N, LanguageCode=en-US, VocabularyFileUri='s3://<never-created-bucket>/phrases.txt'). Actual: HTTP 200 with {'VocabularyName': <N>, 'LanguageCode': 'en-US', 'VocabularyState': 'PENDING', 'LastModifiedTime': <ts>} — the call was accepted and the vocabulary created (assertion: "expected the call to be rejected, but it succeeded and returned ..."). The fixed text of that message carries no request values; only the request's VocabularyName/URI are inputs.
Cause 2: expected the French-audio job's TranscriptionJob.LanguageCode to be 'fr-FR'; actual 'en-US' (assertion: "expected fr-FR to be identified, got en-US"). Both legs ran in the same job set with IdentifyLanguage=True and LanguageOptions=['en-US','es-ES','fr-FR']; the en-US leg passed, so the value is a constant rather than a detection result.

## Steps to Reproduce
Cause 1: CreateVocabulary(VocabularyName=<new>, LanguageCode='en-US', VocabularyFileUri='s3://<bucket-that-was-never-created>/phrases.txt'). Expect BadRequestException and no resource; observe HTTP 200 with VocabularyState=PENDING, and a subsequent GetVocabulary on that name succeeding.
Cause 2: synthesize English and French speech (Polly SynthesizeSpeech, voices Joanna and Lea), put both in S3, StartTranscriptionJob twice with IdentifyLanguage=True, LanguageOptions=['en-US','es-ES','fr-FR'], MediaFormat='mp3'; poll GetTranscriptionJob to COMPLETED and compare TranscriptionJob.LanguageCode for the two jobs — both come back en-US.
Replay: the two check ids in trace_ids.
