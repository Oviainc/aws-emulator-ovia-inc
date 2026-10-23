Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.batch-detect-sentiment-partial-failure-per-index comprehend.batch-detect-partial-failure-oversized-item comprehend.batch-detect-dominant-language-partial-failure-isolates-bad-document comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected comprehend.batch-detect-entities-batch-size-limit-rejected comprehend.batch-size-limit-exceeded-rejected comprehend.detect-targeted-sentiment-unsupported-language-rejected comprehend.detect-dominant-language-ranks-english-highest
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend.batch-detect-sentiment-partial-failure-per-index|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: comprehend.batch-detect-partial-failure-oversized-item|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: comprehend.batch-detect-dominant-language-partial-failure-isolates-bad-document|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.batch-detect-entities-batch-size-limit-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.batch-size-limit-exceeded-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.detect-targeted-sentiment-unsupported-language-rejected|run#4|AssertionError|assert code == "ValidationException", f"unexpected error code: {code!r}"
Issue-Key: comprehend.detect-dominant-language-ranks-english-highest|run#7|AssertionError|assert languages[0]["Score"] > 0.5, languages
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# Comprehend sync inference fidelity: oversized batch documents accepted per-index, wrong refusal class for unsupported language, batch-limit message and flat language scores (4 causes)

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectSentiment

## Description
Four distinct root causes in the Comprehend synchronous (real-time) inference surface. All four were replayed against the booted emulator; comprehend is bootstrapped and answering, so each of these is a fidelity defect, not a bootstrap gap.

Cause 1 of 4: a batch containing one oversized document is accepted with HTTP 200 and a per-index ErrorList instead of being refused wholesale.
AWS: BatchDetectSentiment and BatchDetectDominantLanguage both model TextSizeLimitExceededException (Smithy: BatchDetectSentiment / BatchDetectDominantLanguage errors = BatchSizeLimitExceededException, InternalServerException, InvalidRequestException, TextSizeLimitExceededException [+ UnsupportedLanguageException for sentiment]; errorKind client, httpError 400). Recorded behavior: the per-document size limit (5000 bytes for real-time analysis) is applied to the WHOLE request, so a TextList that mixes one well-formed document with one oversized document is rejected in its entirety with a 400 TextSizeLimitExceededException and NO ResultList/ErrorList body at all. The recorded message is built from the request and is therefore a requirement in rule form: "Input text size exceeds limit. Max length of request text allowed is 5000 bytes while in this request the text size is <byte-length-of-the-offending-document> bytes". The identical well-formed document submitted alone succeeds.
Emulator: the call SUCCEEDS with HTTP 200, returning ResultList with the good document's analysis at Index 0 and a per-index ErrorList entry {"Index":1,"ErrorCode":"TEXT_SIZE_LIMIT_EXCEEDED",...} for the oversized one — i.e. it treats a size violation as a per-item batch error rather than a request-level refusal. Reproduced on BatchDetectSentiment (6000-byte and 5001-byte documents) and on BatchDetectDominantLanguage (300000-byte document).
Why: a client that relies on the all-or-nothing contract reads the 200 as "the whole batch was analyzed", never raises, and silently processes a partial result set; error-handling paths keyed on TextSizeLimitExceededException never run, and index alignment between the submitted TextList and ResultList is assumed intact when it is not.
Fix: the per-document size validation in the BatchDetectSentiment and BatchDetectDominantLanguage handlers must run as a request-level precondition that refuses the whole request, before any per-item analysis or ErrorList construction (services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/batch_detect_sentiment.rs and .../batch_detect_dominant_language.rs).
Traces: comprehend.batch-detect-sentiment-partial-failure-per-index, comprehend.batch-detect-partial-failure-oversized-item, comprehend.batch-detect-dominant-language-partial-failure-isolates-bad-document.

Cause 2 of 4: BatchSizeLimitExceededException carries a request-derived message where AWS uses a fixed one, and the emulator is internally inconsistent about it.
AWS: Smithy models BatchSizeLimitExceededException (errorKind client, httpError 400) on BatchDetectEntities and BatchDetectDominantLanguage. Recorded behavior: for a TextList over the 25-document batch quota the service answers that error with a CONSTANT message that does not echo the request — "Please use a smaller number of items in a single batch. Maximum batch size is 25." — identically across batch operations. The rule here is "constant text, no request-derived substitution", which is what makes the difference reportable rather than incidental.
Emulator: the error code is correct, but the message is synthesized from the request and differs per operation: BatchDetectEntities answers "TextList contains 26 documents, which exceeds the limit of 25. Please use a smaller number of items in a single batch." while BatchDetectDominantLanguage answers "The number of documents in the request (26) exceeds the limit of 25. Try your request again with fewer documents." (examples at 26 documents).
Why: callers and SDK-level log/metric scrapers that match on the service's fixed message string see neither form; the two different emulator templates also mean the same quota violation looks like two different conditions depending on which batch operation was called.
Fix: emit the single fixed BatchSizeLimitExceededException message from the shared batch-size validation used by the batch handlers, instead of formatting a per-operation string (.../src/ops/batch_detect_entities.rs, .../src/ops/batch_detect_dominant_language.rs).
Traces: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected, comprehend.batch-detect-entities-batch-size-limit-rejected, comprehend.batch-size-limit-exceeded-rejected.

Cause 3 of 4: DetectTargetedSentiment refuses an unsupported LanguageCode with the wrong error class.
AWS: DetectTargetedSentiment supports English only. Smithy models UnsupportedLanguageException on the operation (httpError 400), but recorded behavior shows the service refuses a non-English LanguageCode with ValidationException (HTTP 400) — an operation-scoped enum-constraint rejection, not the modeled UnsupportedLanguageException. The message is built from the request (member path, offending value, allowed set) and so is a requirement in rule form: "Value '<value>' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en]" (note AWS's missing space before "failed"); recorded with LanguageCode='fr'.
Emulator: answers error code UnsupportedLanguageException for LanguageCode='fr'.
Why: a client catching ValidationException (or branching on it to surface "this language is not supported for targeted sentiment") sees an unhandled UnsupportedLanguageException instead; retry/classification logic that treats the two classes differently takes the wrong branch, and the enum-constraint message naming the single allowed value is lost.
Fix: the LanguageCode precondition in the DetectTargetedSentiment handler's validation must raise the operation's ValidationException with the enum-constraint message, not the modeled UnsupportedLanguageException (.../src/ops/detect_targeted_sentiment.rs).
Traces: comprehend.detect-targeted-sentiment-unsupported-language-rejected.

Cause 4 of 4: DetectDominantLanguage confidence scores are too flat for a clearly dominant language.
AWS: DetectDominantLanguage returns Languages sorted descending with the dominant language first, and for text that is predominantly one language the leading entry carries a decisive score (recorded behavior: >0.5 for predominantly-English text that also contains short foreign-language greetings).
Emulator: ranking and descending sort are correct, but the leading score is below the decision threshold — observed Languages = [en 0.4687, de 0.4004, fr 0.0496, it 0.0444, es 0.0254, pt 0.0114] for a predominantly English sentence, i.e. the distribution is spread almost evenly between the top two candidates. (Scores are synthesized by the emulator, not sampled from a live model; the divergence is the shape of that distribution, not any one float.)
Why: clients commonly gate on the top score (accept the detection only above a confidence threshold, typically 0.5-0.9) and will reject or mark as ambiguous a document AWS reports confidently, making an otherwise-correct detection unusable.
Fix: the score synthesis behind DetectDominantLanguage (and the batch variant that shares it) should concentrate mass on the dominant candidate rather than splitting it near-evenly across the top candidates (.../src/ops/detect_dominant_language.rs).
Traces: comprehend.detect-dominant-language-ranks-english-highest.

## Evidence
Cause 1 — expected: 400 ClientError TextSizeLimitExceededException, message "Input text size exceeds limit. Max length of request text allowed is 5000 bytes while in this request the text size is 6000 bytes"; actual: HTTP 200 {'ResultList': [{'Index': 0, 'Sentiment': 'NEUTRAL', ...}], 'ErrorList': [{'Index': 1, 'ErrorCode': 'TEXT_SIZE_LIMIT_EXCEEDED', 'ErrorMessage': 'Input text size exceeds limit. Max length of request text allowed is 5000 bytes while in this request the text size is 6000 bytes'}]} (harness: "expected a ClientError, but the call SUCCEEDED"). Same for BatchDetectSentiment at 5001 bytes and BatchDetectDominantLanguage at 300000 bytes.

Cause 2 — code BatchSizeLimitExceededException matched; expected message 'Please use a smaller number of items in a single batch. Maximum batch size is 25.'; actual (BatchDetectEntities) 'TextList contains 26 documents, which exceeds the limit of 25. Please use a smaller number of items in a single batch.'; actual (BatchDetectDominantLanguage) 'The number of documents in the request (26) exceeds the limit of 25. Try your request again with fewer documents.'

Cause 3 — expected error code 'ValidationException' with message "Value 'fr' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en]"; actual error code 'UnsupportedLanguageException'. Positive control (LanguageCode='en') was never reached.

Cause 4 — expected Languages[0]['Score'] > 0.5 with Languages[0]['LanguageCode'] == 'en'; actual [{'LanguageCode': 'en', 'Score': 0.4687420129776001}, {'LanguageCode': 'de', 'Score': 0.4004342555999756}, {'LanguageCode': 'fr', 'Score': 0.04962575063109398}, {'LanguageCode': 'it', 'Score': 0.044446349143981934}, {'LanguageCode': 'es', 'Score': 0.025393610820174217}, {'LanguageCode': 'pt', 'Score': 0.011358028277754784}] (order and descending sort were correct).

## Steps to Reproduce
1. Boot the emulator and point a botocore comprehend client at it.
2. Cause 1: comprehend.batch_detect_sentiment(TextList=["Short valid text for sentiment analysis.", "a"*6000], LanguageCode="en") — expect a 400 TextSizeLimitExceededException ClientError; observe HTTP 200 with ResultList[0] plus ErrorList[{Index:1, TEXT_SIZE_LIMIT_EXCEEDED}]. Repeat with "b"*5001, and with comprehend.batch_detect_dominant_language(TextList=[good_text, "x"*300000]).
3. Cause 2: comprehend.batch_detect_entities(TextList=["A short valid document about Amazon Web Services."]*26, LanguageCode="en") and comprehend.batch_detect_dominant_language(TextList=[f"Document number {i} for language analysis." for i in range(26)]) — compare Error.Message against the fixed AWS string.
4. Cause 3: comprehend.detect_targeted_sentiment(Text="La batterie de cet ordinateur est incroyable, mais l'ecran est decevant.", LanguageCode="fr") — expect ValidationException; observe UnsupportedLanguageException.
5. Cause 4: comprehend.detect_dominant_language(Text="Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer in Seattle.") — inspect Languages[0]['Score'].
6. Re-replay the checks listed in trace_ids to confirm.
