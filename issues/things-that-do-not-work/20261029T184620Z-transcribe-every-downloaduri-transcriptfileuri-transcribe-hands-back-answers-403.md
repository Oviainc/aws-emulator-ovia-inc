Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.create-vocabulary-duplicate-name-conflict transcribe.vocabulary-filter-content-roundtrip-via-download-uri transcribe.update-vocabulary-filter-words-reflected-in-download transcribe.update-vocabulary-resets-state-and-content transcribe.transcription-job-lifecycle-completes transcribe.native-speech-to-text-round-trip-via-polly
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.create-vocabulary-duplicate-name-conflict|_download#11|HTTPError|with urllib.request.urlopen(_routed_request(ctx, url), timeout=30) as resp:
Issue-Key: transcribe.vocabulary-filter-content-roundtrip-via-download-uri|_download#6|HTTPError|with urllib.request.urlopen(_routed_request(ctx, url), timeout=30) as resp:
Issue-Key: transcribe.update-vocabulary-filter-words-reflected-in-download|_download#6|HTTPError|with urllib.request.urlopen(_routed_request(ctx, url), timeout=30) as resp:
Issue-Key: transcribe.update-vocabulary-resets-state-and-content|_download#7|HTTPError|with urllib.request.urlopen(_routed_request(ctx, url), timeout=30) as resp:
Issue-Key: transcribe.transcription-job-lifecycle-completes|run#14|HTTPError|with urllib.request.urlopen(_routed_request(ctx, uri), timeout=30) as resp:
Issue-Key: transcribe.native-speech-to-text-round-trip-via-polly|run#16|HTTPError|with urllib.request.urlopen(_routed_request(ctx, uri), timeout=30) as resp:
Issue-Commit: 96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9

# Every DownloadUri / TranscriptFileUri Transcribe hands back answers 403 Forbidden, so no vocabulary, filter or transcript content can be read

> _Note: this issue was identified against the code base at commit `96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVocabulary

## Description
Cause 1 of 1 — the service-managed download URIs Transcribe returns are not fetchable.

AWS: GetVocabulary.DownloadUri, GetVocabularyFilter.DownloadUri and TranscriptionJob.Transcript.TranscriptFileUri are modeled as Uri (@length 1..2000, @pattern ^(s3://|http(s*)://).+$). The Transcript doc states the contract explicitly: "The Amazon S3 location of your transcript. You can use this URI to access or download your transcript... If you didn't include OutputBucketName in your transcription job request, your transcript is stored in a service-managed bucket, and TranscriptFileUri provides you with a temporary URI you can use for secure access to your transcript", temporary URIs being "only valid for 15 minutes" and yielding an AccessDenied only once expired. So a GET of a freshly minted URI MUST return 200 with the content: the vocabulary's phrase list, the filter's word list, or the transcript JSON document (jobName, status, results.transcripts[0].transcript, results.items).
Emulator: every such GET fails immediately with HTTP 403 Forbidden (urllib.error.HTTPError: HTTP Error 403: Forbidden) on the first attempt, seconds after the URI was handed out — long inside any 15-minute validity window. This reproduces across all three URI families (vocabulary DownloadUri, vocabulary-filter DownloadUri, transcript TranscriptFileUri), so the URI the emulator mints is either signed with a credential/scope the serving endpoint rejects or points at a path that endpoint does not authorize. The control-plane calls that produce the URI all succeed (200), so the defect is in minting/serving the URI, not in the Get* operations' shapes.
Why: the entire Transcribe data plane is unreachable through the emulator. A client cannot read a custom vocabulary's content, cannot read a filter's word list, and cannot obtain any transcript at all — the job reports COMPLETED while its output is undownloadable. Round-trip verification (did my words/phrases persist? did my update take effect?) and every real consumer of a transcript break, and the failure arrives as a bare 403 rather than the modeled AccessDenied-on-expiry, so clients cannot even retry correctly by re-issuing GetTranscriptionJob.
Fix: the URI-minting code behind GetVocabulary/GetVocabularyFilter DownloadUri and TranscriptionJob.Transcript.TranscriptFileUri in the transcribe crate, together with the endpoint that serves those download paths (its authorization of the minted URI). Both sides must agree so that a GET issued right after the URI is returned yields 200 and the stored content.
Traces: the six check ids in trace_ids; each fails at its first urlopen of a URI the emulator itself just returned.

## Evidence
Expected: GET of a just-returned DownloadUri / TranscriptFileUri returns 200 and the content (e.g. the submitted words DANG/HECK for a filter, the phrase ALDERAAN for a vocabulary, a transcript JSON whose jobName equals the job and whose status is COMPLETED).
Actual: urllib.error.HTTPError: HTTP Error 403: Forbidden, raised at request.py line 639 (http_error_default) on the first fetch, for all of:
  - transcribe.create-vocabulary-duplicate-name-conflict (vocabulary DownloadUri, after the ConflictException assertions had already passed)
  - transcribe.vocabulary-filter-content-roundtrip-via-download-uri (filter DownloadUri)
  - transcribe.update-vocabulary-filter-words-reflected-in-download (filter DownloadUri)
  - transcribe.update-vocabulary-resets-state-and-content (vocabulary DownloadUri)
  - transcribe.transcription-job-lifecycle-completes (Transcript.TranscriptFileUri)
  - transcribe.native-speech-to-text-round-trip-via-polly (Transcript.TranscriptFileUri)
The URI itself was non-empty and satisfied the modeled Uri pattern (the checks poll until DownloadUri is present and only then fetch), so the 403 comes from serving it, not from a missing field.

## Steps to Reproduce
1. CreateVocabularyFilter(VocabularyFilterName=F, LanguageCode=en-US, Words=["dang","heck"]), poll GetVocabularyFilter until DownloadUri is present, then HTTP GET that URI -> 403 instead of 200 + the word list.
2. CreateVocabulary(VocabularyName=V, LanguageCode=en-US, Phrases=["Alderaan"]), poll GetVocabulary until VocabularyState=READY, HTTP GET DownloadUri -> 403 instead of 200 + the phrase list.
3. StartTranscriptionJob with no OutputBucketName, poll GetTranscriptionJob until COMPLETED, HTTP GET TranscriptionJob.Transcript.TranscriptFileUri -> 403 instead of 200 + the transcript JSON document.
Replay: the six check ids in trace_ids.
