Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.detect-sentiment-text-size-limit-exceeded-rejected comprehend.detect-sentiment-text-size-limit-rejected comprehend.text-size-limit-exceeded-rejected comprehend.detect-dominant-language-ranks-english-highest
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend.detect-sentiment-text-size-limit-exceeded-rejected|_error#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: comprehend.detect-sentiment-text-size-limit-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.text-size-limit-exceeded-rejected|run#7|AssertionError|assert message == (
Issue-Key: comprehend.detect-dominant-language-ranks-english-highest|run#5|AssertionError|assert len(languages) >= 2, f"expected multiple ranked candidates, got {languages!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Comprehend synchronous inference: text-size quotas are wrong (oversized documents accepted), refusal messages are bespoke, and DetectDominantLanguage returns a single wrong candidate (4 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectSentiment

## Description
Four distinct root causes in Comprehend's synchronous (real-time) inference handlers. All four are grounded in failing behavioral checks of this suite; all live in the comprehend service crate services/service-com-amazonaws-comprehend-comprehend-20171127.

Cause 1 of 4 — DetectSentiment enforces a 5120-byte document quota instead of AWS's 5000-byte quota, so documents AWS refuses are accepted.
AWS: Smithy: DetectSentiment.Text -> CustomerInputString, @required, @length min 1, @sensitive; modeled error TextSizeLimitExceededException (HTTP 400, errorKind client). The model states no byte maximum, so the quota itself is recorded behavior only: the recording shows a 5001-byte Text rejected with HTTP 400 TextSizeLimitExceededException, i.e. the enforced real-time maximum is 5000 bytes of UTF-8.
Emulator: a 5001-byte Text returns HTTP 200 with {'Sentiment': 'NEUTRAL', 'SentimentScore': {...}}. The emulator's own refusal message (see Cause 2) states its threshold is 5120 bytes, so every document between 5001 and 5120 bytes is accepted where AWS returns 400.
Why: a client that relies on AWS refusing oversized documents (chunking logic, size pre-checks, error-path tests) sees a success it can never see in production, and an input size that AWS rejects silently produces a fabricated NEUTRAL sentiment result.
Fix: the DetectSentiment handler's input validation — the per-document size quota constant must be 5000 bytes of UTF-8, not 5120.
Traces: comprehend.detect-sentiment-text-size-limit-exceeded-rejected.

Cause 2 of 4 — the TextSizeLimitExceededException message is a bespoke emulator sentence instead of AWS's size-report sentence built from the request.
AWS: recorded behavior only, but the message is request-derived and therefore a rule, not an example: AWS reports 'Input text size exceeds limit. Max length of request text allowed is <limit> bytes while in this request the text size is <actual> bytes', where <limit> is the operation's quota and <actual> is the UTF-8 byte length of the submitted text. Both recordings in this suite (DetectSentiment at 9200 bytes, DetectDominantLanguage at 200000 bytes) show exactly that form with HTTP 400 / TextSizeLimitExceededException.
Emulator: returns its own sentence naming the operation and its own limit, e.g. 'Text is 9200 bytes, which exceeds the 5120 byte limit for DetectSentiment. Use a smaller document.' and 'Text is 200000 bytes, which exceeds the 102400 byte limit for DetectDominantLanguage. Use a smaller document.' (HTTP status and error code are correct in both.)
Why: clients and SDK test suites that parse the limit/actual pair out of the message (a common pattern for adaptive chunking) extract nothing, and the message also advertises the wrong quota.
Fix: the shared text-size validation used by the synchronous inference handlers — emit AWS's 'Input text size exceeds limit. Max length of request text allowed is N bytes while in this request the text size is M bytes' form, parameterized on the operation's real quota and the request's byte length.
Traces: comprehend.detect-sentiment-text-size-limit-rejected, comprehend.text-size-limit-exceeded-rejected.

Cause 3 of 4 — DetectDominantLanguage enforces a 102400-byte quota instead of AWS's 100000-byte quota.
AWS: Smithy: DetectDominantLanguage.Text -> CustomerInputString, @required, @length min 1, @sensitive; modeled error TextSizeLimitExceededException (HTTP 400). The byte maximum is recorded behavior only: the recording shows AWS reporting 'Max length of request text allowed is 100000 bytes'.
Emulator: the refusal message reports its threshold as 102400 bytes (1024*100), so documents of 100001..102400 bytes are accepted where AWS returns HTTP 400 TextSizeLimitExceededException.
Why: same overly-permissive hazard as Cause 1, in the 2.4 KB band just above AWS's limit — an emulator-green client fails in production.
Fix: the DetectDominantLanguage handler's input validation — the quota constant must be 100000 bytes, not 102400.
Traces: comprehend.text-size-limit-exceeded-rejected.

Cause 4 of 4 — DetectDominantLanguage returns exactly one, incorrect candidate instead of a score-ranked list with the dominant language first.
AWS: Smithy: DetectDominantLanguage output member Languages -> ListOfDominantLanguages (list of DominantLanguage{LanguageCode, Score}); the operation's documentation states the array is ordered so that the dominant language is the first element. Recorded behavior: a text mixing an English sentence with short greetings in several other languages returns two or more DominantLanguage entries sorted by descending Score with LanguageCode 'en' first at Score > 0.5.
Emulator: returns a single entry, and the wrong one: [{'LanguageCode': 'fr', 'Score': 0.4426087737083435}] for 'Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer in Seattle.' — no second candidate exists, so the documented descending-score ordering contract is unobservable, and the top language is not the dominant one.
Why: any client branching on Languages[0].LanguageCode routes predominantly-English content down a non-English path, and clients that inspect runner-up candidates/confidence always see a one-element list.
Fix: the DetectDominantLanguage handler's detection logic and response projection — produce multiple scored candidates and emit them sorted by descending Score with the dominant language first.
Traces: comprehend.detect-dominant-language-ranks-english-highest.

## Evidence
Cause 1: expected HTTP 400 TextSizeLimitExceededException for Text of 5001 bytes; got HTTP 200 {'Sentiment': 'NEUTRAL', 'SentimentScore': {'Positive': 0.0, 'Negative': 0.0, 'Neutral': 1.0, 'Mixed': 0.0}}.
Cause 2: DetectSentiment, 9200-byte text — expected message 'Input text size exceeds limit. Max length of request text allowed is 5000 bytes while in this request the text size is 9200 bytes'; got 'Text is 9200 bytes, which exceeds the 5120 byte limit for DetectSentiment. Use a smaller document.' DetectDominantLanguage, 200000-byte text — expected 'Input text size exceeds limit. Max length of request text allowed is 100000 bytes while in this request the text size is 200000 bytes'; got 'Text is 200000 bytes, which exceeds the 102400 byte limit for DetectDominantLanguage. Use a smaller document.'
Cause 3: emulator's own refusal text names a 102400-byte limit for DetectDominantLanguage where AWS reports 100000.
Cause 4: expected >= 2 ranked candidates with Languages[0].LanguageCode == 'en' and Score > 0.5; got [{'LanguageCode': 'fr', 'Score': 0.4426087737083435}].

## Steps to Reproduce
1. Boot the emulator and point a botocore comprehend client at it.
2. Cause 1: comprehend.detect_sentiment(Text='x'*5001, LanguageCode='en') — emulator returns 200, AWS returns 400 TextSizeLimitExceededException.
3. Cause 2: comprehend.detect_sentiment(Text='x'*9200, LanguageCode='en') and comprehend.detect_dominant_language(Text='word '*40000) — compare Error.Message against AWS's 'Input text size exceeds limit. Max length of request text allowed is N bytes while in this request the text size is M bytes'.
4. Cause 3: comprehend.detect_dominant_language(Text='x'*100001) — emulator accepts, AWS returns 400 TextSizeLimitExceededException.
5. Cause 4: comprehend.detect_dominant_language(Text='Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer in Seattle.') and inspect Languages: expect multiple entries sorted by descending Score with 'en' first.
6. Replay: comprehend.detect-sentiment-text-size-limit-exceeded-rejected, comprehend.detect-sentiment-text-size-limit-rejected, comprehend.text-size-limit-exceeded-rejected, comprehend.detect-dominant-language-ranks-english-highest.
