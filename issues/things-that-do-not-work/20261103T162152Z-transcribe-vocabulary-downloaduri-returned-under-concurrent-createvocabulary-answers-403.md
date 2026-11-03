Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.concurrent-create-vocabulary-same-name-one-wins
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.concurrent-create-vocabulary-same-name-one-wins|_download#14|HTTPError|with urllib.request.urlopen(_routed_request(ctx, url), timeout=30) as resp:
Issue-Commit: 75da5c853dde52b5c6431a41dd7ff04ce793da30

# Vocabulary DownloadUri returned under concurrent CreateVocabulary answers 403 Forbidden, so the race winner's stored content cannot be read

> _Note: this issue was identified against the code base at commit `75da5c853dde52b5c6431a41dd7ff04ce793da30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVocabulary

## Description
Cause 1 of 1: the service-managed vocabulary DownloadUri that GetVocabulary hands back is not fetchable — a GET of the URI the emulator itself just minted is refused with HTTP 403 Forbidden.

Related open issue: 20261029T184620Z-transcribe-every-downloaduri-transcriptfileuri-transcribe-hands-back-answers-403.md describes the same family of refusal, but its Issue-Trace-Ids / Issue-Key set does NOT carry transcribe.concurrent-create-vocabulary-same-name-one-wins, so this check is not covered by it and is filed here. A fixer should treat it as the same minting/serving surface and confirm with the replay key listed below.

AWS: Smithy: GetVocabulary output member DownloadUri targets Uri with @length min 1 / max 2000 and @pattern ^(s3://|http(s*)://).+$; GetVocabulary's modeled errors are only BadRequestException (400), InternalFailureException (500), LimitExceededException (429) and NotFoundException (404) — 403 is not among them. The DownloadUri contract is that it is a temporary, pre-authorized URI the caller may fetch directly: a GET issued immediately after the vocabulary reaches VocabularyState=READY MUST return 200 with the vocabulary's stored phrase list as its body. AWS only refuses such a URI (AccessDenied) once the temporary grant has expired, which cannot be the case seconds after it was issued. CreateVocabulary's ConflictException (409) part of this check behaved correctly — exactly one of eight concurrent CreateVocabulary calls with one VocabularyName succeeded and the other seven returned ConflictException with the message "The requested job name already exists. Use a different job name." (that whole string is fixed text for this error class; it carries no request-derived values) — so the divergence is isolated to serving the download URI.

Emulator: the first and only HTTP GET of the DownloadUri returned by GetVocabulary fails with urllib.error.HTTPError: HTTP Error 403: Forbidden (raised in urllib request.py http_error_default, line 639). The control-plane calls all succeeded: CreateVocabulary returned, GetVocabulary polled to VocabularyState=READY, and DownloadUri was present and matched the modeled Uri pattern. So the emulator either signs/scopes the URI with a credential the serving endpoint rejects, or points it at a path that endpoint does not authorize — the two sides disagree. Note the refusal is a bare 403 with no AWS error code or message body at all, where AWS would answer 200 with content.

Why: a client that relies on AWS behavior cannot read a custom vocabulary's content at all. In particular it cannot verify which writer won a concurrent CreateVocabulary race: the API reports one winner and seven conflicts, but the only way to observe WHICH phrase list actually persisted (and that no loser's phrases leaked in via a lost update) is the DownloadUri, and that read is impossible. Any round-trip verification of vocabulary content, and any consumer that downloads a vocabulary, breaks. Because the refusal arrives as a bare 403 instead of a modeled error, clients cannot distinguish an expired grant (retry by re-issuing GetVocabulary) from a permanent failure.

Fix: the URI-minting code behind GetVocabulary's DownloadUri in the transcribe service crate, together with the endpoint/handler that serves those download paths and authorizes the minted URI. Both must agree so a GET issued right after the URI is handed out returns 200 and the stored phrase list.

Traces: transcribe.concurrent-create-vocabulary-same-name-one-wins — the race and ConflictException assertions pass, then the check dies on its first urlopen of the DownloadUri GetVocabulary returned.

## Evidence
Expected: HTTP GET of the DownloadUri returned by GetVocabulary (vocabulary in state READY) returns 200 and a body containing the winning racer's phrase (and none of the losing racers' phrases).
Actual: urllib.error.HTTPError: HTTP Error 403: Forbidden, raised at urllib request.py line 639 (http_error_default) via _call_chain at line 492, on the first fetch of that URI.

The concurrency portion of the same check passed before the download: exactly one of 8 concurrent CreateVocabulary calls on one VocabularyName succeeded, the other 7 returned Error.Code=ConflictException with Error.Message="The requested job name already exists. Use a different job name." GetVocabulary then reported VocabularyState=READY and a non-empty DownloadUri satisfying ^(s3://|http(s*)://).+$ — so the 403 comes from serving the URI, not from a missing or malformed field.

## Steps to Reproduce
1. Pick one VocabularyName N. From 8 threads concurrently call CreateVocabulary(VocabularyName=N, LanguageCode="en-US", Phrases=[<one distinct letters-only word per thread>], Tags=[{Key:...,Value:...}]).
2. Observe exactly one success and seven ConflictException(409) responses (this part currently behaves correctly).
3. Poll GetVocabulary(VocabularyName=N) until VocabularyState != "PENDING"; it reaches READY.
4. HTTP GET the DownloadUri from that GetVocabulary response.
   Expected: 200 with a body containing the winning thread's phrase and no losing thread's phrase.
   Actual: HTTP 403 Forbidden with no AWS error code or message.
Replay: transcribe.concurrent-create-vocabulary-same-name-one-wins
