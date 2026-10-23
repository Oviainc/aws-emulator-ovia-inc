Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.get-speech-synthesis-task-not-found-rejected polly.get-speech-synthesis-task-invalid-id-format-rejected polly.put-lexicon-max-lexeme-length-exceeded-rejected polly.put-lexicon-unsupported-alphabet-error polly.synthesize-speech-engine-not-supported-rejected polly.delete-lexicon-removes-from-list polly.get-lexicon-not-found-after-delete polly.get-lexicon-not-found-error-code polly.list-lexicons-reflects-created polly.synthesize-speech-lexicon-not-found-rejected polly.describe-voices-invalid-next-token-rejected polly.list-lexicons-invalid-next-token-error polly.max-lexicons-number-exceeded-rejected polly.put-lexicon-size-exceeded-rejected polly.put-lexicon-invalid-pls-xml-rejected polly.put-lexicon-unsupported-language-error polly.synthesize-speech-invalid-sample-rate-error polly.synthesize-speech-invalid-ssml-error polly.synthesize-speech-marks-not-supported-for-format-rejected polly.synthesize-speech-ssml-marks-not-supported-for-text-type-error polly.synthesize-speech-text-length-exceeded-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.get-speech-synthesis-task-not-found-rejected|run#10|AssertionError|assert code_missing == "SynthesisTaskNotFoundException", code_missing
Issue-Key: polly.get-speech-synthesis-task-invalid-id-format-rejected|run#9|AssertionError|assert unknown_code == "SynthesisTaskNotFoundException", unknown_code
Issue-Key: polly.put-lexicon-max-lexeme-length-exceeded-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: polly.put-lexicon-unsupported-alphabet-error|run#7|AssertionError|assert code == "InvalidLexiconException", code
Issue-Key: polly.synthesize-speech-engine-not-supported-rejected|run#4|AssertionError|assert code == "ValidationException", code
Issue-Key: polly.delete-lexicon-removes-from-list|run#17|AssertionError|assert message == "Lexicon not found", message
Issue-Key: polly.get-lexicon-not-found-after-delete|run#10|AssertionError|assert message_get == "Lexicon not found", message_get
Issue-Key: polly.get-lexicon-not-found-error-code|run#9|AssertionError|assert message == "Lexicon not found", message
Issue-Key: polly.list-lexicons-reflects-created|run#13|AssertionError|assert message == "Lexicon not found", message
Issue-Key: polly.synthesize-speech-lexicon-not-found-rejected|run#5|AssertionError|assert message == "Lexicon not found", message
Issue-Key: polly.describe-voices-invalid-next-token-rejected|run#5|AssertionError|assert message == "Invalid NextToken parameter", message
Issue-Key: polly.list-lexicons-invalid-next-token-error|run#7|AssertionError|assert message == "Invalid NextToken parameter", message
Issue-Key: polly.max-lexicons-number-exceeded-rejected|run#11|AssertionError|assert message == "Max lexicon number has been exceeded", message
Issue-Key: polly.put-lexicon-size-exceeded-rejected|run#7|AssertionError|assert message == "Maximum lexicons size has been exceeded", message
Issue-Key: polly.put-lexicon-invalid-pls-xml-rejected|run#6|AssertionError|assert message == "Invalid PLS Lexicon", message
Issue-Key: polly.put-lexicon-unsupported-language-error|run#7|AssertionError|assert message == "Unsupported PLS language", message
Issue-Key: polly.synthesize-speech-invalid-sample-rate-error|run#5|AssertionError|assert message == "Invalid SampleRate parameter", message
Issue-Key: polly.synthesize-speech-invalid-ssml-error|run#6|AssertionError|assert message == "Invalid SSML request", f"expected the exact invalid-SSML message, got {message!r}"
Issue-Key: polly.synthesize-speech-marks-not-supported-for-format-rejected|run#5|AssertionError|assert message == "Speech marks are not supported for this format", message
Issue-Key: polly.synthesize-speech-ssml-marks-not-supported-for-text-type-error|run#4|AssertionError|assert message == "SSML Speech marks are not supported for this text type", message
Issue-Key: polly.synthesize-speech-text-length-exceeded-rejected|run#6|AssertionError|assert message == "Maximum text length has been exceeded", message
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Polly error semantics: GetSpeechSynthesisTask is a 501 stub, PutLexicon skips the lexeme-length quota, two refusals use the wrong error code, and client-error messages are bespoke prose (5 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSpeechSynthesisTask

## Description
Five distinct root causes in Polly's rejection/error-reporting paths. All are polly-service causes; each is listed with its own failing check ids.

Cause 1 of 5: GetSpeechSynthesisTask answers HTTP 501 NotImplemented for every request that passes request-shape validation.
AWS: Smithy: GetSpeechSynthesisTask GET /v1/synthesisTasks/{TaskId}, TaskId @pattern ^[a-zA-Z0-9_-]{1,100}$, output SynthesisTask, modeled errors InvalidTaskIdException (400), SynthesisTaskNotFoundException (400) and ServiceFailureException (500). A well-formed TaskId that names no task must be refused with SynthesisTaskNotFoundException (HTTP 400); recorded behavior shows that error with a null message body. NotImplemented is not a modeled error for this operation.
Emulator: the handler is still a scaffold — services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs returns Err(crate::scaffolded::Scaffolded::not_implemented("GetSpeechSynthesisTask")) (file carries the "SCAFFOLDED, NOT IMPLEMENTED" marker), so a TaskId of "a"*40 yields error code NotImplemented instead of SynthesisTaskNotFoundException. The framework's @pattern rejection still works, so only the post-validation path is affected. NOTE: no open issue names GetSpeechSynthesisTask — the suite's auto-filed stub issues cover StartSpeechSynthesisTask and ListSpeechSynthesisTasks only, and this operation is reached through a caught ClientError so it was not auto-detected.
Why: a client polling a task cannot distinguish "no such task" from a server fault, and SDK retry logic treats the 501 as retryable; the whole asynchronous-synthesis read path is unusable.
Fix: implement the GetSpeechSynthesisTask handler (that file) over the synthesis-task state and delete its SCAFFOLDED marker.
Traces: polly.get-speech-synthesis-task-not-found-rejected, polly.get-speech-synthesis-task-invalid-id-format-rejected.

Cause 2 of 5: PutLexicon accepts a lexeme whose replacement exceeds the lexeme-length quota.
AWS: Smithy: PutLexicon models MaxLexemeLengthExceededException (HTTP 400), documented as "The maximum size of the lexeme would be exceeded by this operation." Recorded behavior: a PLS <lexeme> whose <alias> is 150 characters (over the documented 100-character limit) is refused with that error and the lexicon is not stored.
Emulator: the call returns HTTP 200 with an empty body and the lexicon is created.
Why: the quota is unenforced, so a client that relies on Polly refusing an oversized lexeme stores a document real AWS rejects — the divergence only surfaces in production.
Fix: lexeme-length validation in the PutLexicon handler's PLS parsing/validation step (services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs).
Traces: polly.put-lexicon-max-lexeme-length-exceeded-rejected.

Cause 3 of 5: PutLexicon with an unrecognized PLS alphabet returns the wrong error code.
AWS: Smithy: PutLexicon models both InvalidLexiconException and UnsupportedPlsAlphabetException (both HTTP 400). Recorded behavior: a PLS document declaring alphabet="klingon" is refused with code InvalidLexiconException — Polly's PLS parser rejects the document as a whole rather than isolating the more specific modeled alphabet error.
Emulator: HTTP 400 with code UnsupportedPlsAlphabetException.
Why: clients branch on the error code; code-specific handling and retry classification for malformed lexicon documents takes the wrong branch.
Fix: error classification in the PutLexicon handler's alphabet-attribute check.
Traces: polly.put-lexicon-unsupported-alphabet-error.

Cause 4 of 5: SynthesizeSpeech voice/engine incompatibility returns the wrong error code.
AWS: Smithy: SynthesizeSpeech models EngineNotSupportedException (HTTP 400), but recorded behavior for Engine=neural on a standard-only voice (Geraint) is the generic request-shape failure: code ValidationException (HTTP 400), message "This voice does not support the selected engine: neural" (recorded example). The modeled EngineNotSupportedException is not what the wire returns.
Emulator: HTTP 400 with code EngineNotSupportedException.
Why: a client that distinguishes request-shape ValidationException from service-specific errors mis-routes this refusal.
Fix: error classification in the SynthesizeSpeech handler's voice/engine compatibility check (synthesize_speech.rs).
Traces: polly.synthesize-speech-engine-not-supported-rejected.

Cause 5 of 5: client-error messages are bespoke emulator prose instead of Polly's fixed per-error message.
AWS: recorded behavior only — the model declares only a generic `message: ErrorMessage` member on each error shape. Across sixteen independent recordings this service returns one FIXED canonical sentence per error code, never one built from the request: LexiconNotFoundException -> "Lexicon not found" (observed in 5 separate checks, with different lexicon names); InvalidNextTokenException -> "Invalid NextToken parameter" (DescribeVoices and ListLexicons); MaxLexiconsNumberExceededException -> "Max lexicon number has been exceeded"; LexiconSizeExceededException -> "Maximum lexicons size has been exceeded"; InvalidLexiconException -> "Invalid PLS Lexicon"; UnsupportedPlsLanguageException -> "Unsupported PLS language"; InvalidSampleRateException -> "Invalid SampleRate parameter"; InvalidSsmlException -> "Invalid SSML request"; MarksNotSupportedForFormatException -> "Speech marks are not supported for this format"; SsmlMarksNotSupportedForTextTypeException -> "SSML Speech marks are not supported for this text type"; TextLengthExceededException -> "Maximum text length has been exceeded". The requirement is the rule (a fixed message per error code, independent of the request), not any one recorded string.
Emulator: every one of these returns the right code and status but a self-authored, request-interpolated sentence, e.g. "Amazon Polly can't find the specified lexicon: ltgtfe7c3e80084a", "the NextToken is invalid: not-a-real-pagination-token", "the maximum size of the specified lexicon would be exceeded by this operation: 40078 characters, maximum 4000", "the specified sample rate is not valid for mp3: 99999; valid values are 8000, 16000, ...".
Why: messages that embed request values are not the fixed strings AWS emits, so clients, tests and log scrapers that match on Polly's message text (and anything redaction-sensitive, since the emulator echoes the caller's lexicon names and tokens back) diverge.
Fix: the shared Polly error-construction/serialization path that builds each modeled exception's message (the per-operation handlers plus the service's error helper) should emit the fixed per-code sentence.
Traces: polly.delete-lexicon-removes-from-list, polly.get-lexicon-not-found-after-delete, polly.get-lexicon-not-found-error-code, polly.list-lexicons-reflects-created, polly.synthesize-speech-lexicon-not-found-rejected, polly.describe-voices-invalid-next-token-rejected, polly.list-lexicons-invalid-next-token-error, polly.max-lexicons-number-exceeded-rejected, polly.put-lexicon-size-exceeded-rejected, polly.put-lexicon-invalid-pls-xml-rejected, polly.put-lexicon-unsupported-language-error, polly.synthesize-speech-invalid-sample-rate-error, polly.synthesize-speech-invalid-ssml-error, polly.synthesize-speech-marks-not-supported-for-format-rejected, polly.synthesize-speech-ssml-marks-not-supported-for-text-type-error, polly.synthesize-speech-text-length-exceeded-rejected.

## Evidence
Cause 1: polly.get-speech-synthesis-task-not-found-rejected and polly.get-speech-synthesis-task-invalid-id-format-rejected both pass the ValidationException leg, then fail at `assert code == "SynthesisTaskNotFoundException"` with AssertionError: NotImplemented for TaskId="a"*40. Handler evidence: services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs lines 3-6 ("SCAFFOLDED, NOT IMPLEMENTED") and line 160 `Err(crate::scaffolded::Scaffolded::not_implemented("GetSpeechSynthesisTask"))`.
Cause 2: polly.put-lexicon-max-lexeme-length-exceeded-rejected — AssertionError: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" for a PLS lexeme with a 150-character <alias>. Expected: 400 MaxLexemeLengthExceededException and no stored lexicon.
Cause 3: polly.put-lexicon-unsupported-alphabet-error — `assert code == "InvalidLexiconException"` failed with AssertionError: UnsupportedPlsAlphabetException for alphabet="klingon".
Cause 4: polly.synthesize-speech-engine-not-supported-rejected — `assert code == "ValidationException"` failed with AssertionError: EngineNotSupportedException for VoiceId=Geraint, Engine=neural.
Cause 5: expected vs actual message, same code and status in every case. Examples: LexiconNotFoundException expected "Lexicon not found", got "Amazon Polly can't find the specified lexicon: ltgtfe7c3e80084a"; InvalidNextTokenException expected "Invalid NextToken parameter", got "the NextToken is invalid, or was not issued for this request's Engine, LanguageCode and IncludeAdditionalLanguageCodes: this-was-never-issued-by-polly==" (DescribeVoices) and "the NextToken is invalid: not-a-real-pagination-token" (ListLexicons); MaxLexiconsNumberExceededException expected "Max lexicon number has been exceeded", got "the maximum number of lexicons would be exceeded by this operation: 100 lexicons are already stored"; LexiconSizeExceededException expected "Maximum lexicons size has been exceeded", got "the maximum size of the specified lexicon would be exceeded by this operation: 40078 characters, maximum 4000"; InvalidLexiconException expected "Invalid PLS Lexicon", got "the lexicon content is not a valid PLS document: the document element is <not-a-lexicon>, not <lexicon>"; UnsupportedPlsLanguageException expected "Unsupported PLS language", got "the language specified in the lexicon is unsupported: xx-ZZ"; InvalidSampleRateException expected "Invalid SampleRate parameter", got "the specified sample rate is not valid for mp3: 99999; valid values are 8000, 16000, 22050, 24000, 44100, 48000"; InvalidSsmlException expected "Invalid SSML request", got "the SSML you provided is invalid; verify the SSML syntax: an SSML element is left unclosed"; MarksNotSupportedForFormatException expected "Speech marks are not supported for this format", got "speech marks are not supported for the OutputFormat selected (mp3); they are only available for content in json format"; SsmlMarksNotSupportedForTextTypeException expected "SSML Speech marks are not supported for this text type", got "SSML speech marks are not supported for plain text-type input"; TextLengthExceededException expected "Maximum text length has been exceeded", got "the value of the Text parameter is longer than the accepted limits: 3001 billed characters, maximum 3000".
Model facts used: smithy_query(operation="GetSpeechSynthesisTask,PutLexicon,SynthesizeSpeech") and smithy_query(shape="MaxLexemeLengthExceededException,UnsupportedPlsAlphabetException,InvalidLexiconException,EngineNotSupportedException,LexiconNotFoundException,SynthesisTaskNotFoundException") on services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json.

## Steps to Reproduce
Against the booted emulator with a botocore polly client:
1. polly.get_speech_synthesis_task(TaskId="a"*40) -> HTTP 501 code NotImplemented; expected HTTP 400 SynthesisTaskNotFoundException (null message). (polly.get_speech_synthesis_task(TaskId="bad id! not/valid") correctly gives ValidationException, so only the post-validation path diverges.)
2. polly.put_lexicon(Name="sim2realX", Content=<PLS with one <lexeme> whose <alias> is "x"*150>) -> HTTP 200 and the lexicon is listed; expected HTTP 400 MaxLexemeLengthExceededException and nothing stored.
3. polly.put_lexicon(Name="sim2realY", Content=<PLS with alphabet="klingon">) -> code UnsupportedPlsAlphabetException; expected InvalidLexiconException.
4. polly.synthesize_speech(Text="Testing engine compatibility validation.", OutputFormat="mp3", VoiceId="Geraint", Engine="neural") -> code EngineNotSupportedException; expected ValidationException.
5. polly.get_lexicon(Name=<never stored>) -> code LexiconNotFoundException with message "Amazon Polly can't find the specified lexicon: <name>"; expected the fixed message "Lexicon not found". Same shape of divergence for the other codes listed in Cause 5.
Replay: the 21 check ids in trace_ids.
