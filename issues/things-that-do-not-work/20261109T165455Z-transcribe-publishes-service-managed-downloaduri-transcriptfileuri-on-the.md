Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.concurrent-create-vocabulary-same-name-one-wins transcribe.create-vocabulary-duplicate-name-conflict transcribe.native-speech-to-text-round-trip-via-polly transcribe.transcription-job-lifecycle-completes transcribe.update-vocabulary-filter-words-reflected-in-download transcribe.update-vocabulary-resets-state-and-content transcribe.vocabulary-filter-content-roundtrip-via-download-uri
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.concurrent-create-vocabulary-same-name-one-wins|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.create-vocabulary-duplicate-name-conflict|_routed_request#11|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.native-speech-to-text-round-trip-via-polly|_routed_request#16|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.transcription-job-lifecycle-completes|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.update-vocabulary-filter-words-reflected-in-download|_routed_request#6|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.update-vocabulary-resets-state-and-content|_routed_request#7|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: transcribe.vocabulary-filter-content-roundtrip-via-download-uri|_routed_request#6|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 67d35df6c1acce919d84587dd79dfbb9f77fdc15

# Transcribe publishes service-managed DownloadUri / TranscriptFileUri on the real AWS S3 host (https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/...), an address the emulator does not serve

> _Note: this issue was identified against the code base at commit `67d35df6c1acce919d84587dd79dfbb9f77fdc15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVocabulary

## Description
Cause 1 of 1: every service-managed download location Transcribe reports is built with Amazon S3's REAL public host instead of an address this emulator answers on, so the location cannot be fetched from a client of the emulator at all. Failing check ids: transcribe.vocabulary-filter-content-roundtrip-via-download-uri, transcribe.update-vocabulary-filter-words-reflected-in-download, transcribe.update-vocabulary-resets-state-and-content, transcribe.create-vocabulary-duplicate-name-conflict, transcribe.concurrent-create-vocabulary-same-name-one-wins, transcribe.transcription-job-lifecycle-completes, transcribe.native-speech-to-text-round-trip-via-polly.

AWS: Smithy (transcribe): GetVocabulary.DownloadUri, GetVocabularyFilter.DownloadUri and Transcript.TranscriptFileUri / Transcript.RedactedTranscriptFileUri all target Uri (@length 1..2000, @pattern ^(s3://|http(s*)://).+$). Transcript's own doc states the contract: "The Amazon S3 location of your transcript. You can use this URI to access or download your transcript... If you didn't include OutputBucketName in your transcription job request, your transcript is stored in a service-managed bucket, and TranscriptFileUri provides you with a temporary URI you can use for secure access to your transcript", temporary URIs being "only valid for 15 minutes" and yielding AccessDenied only once expired. The requirement is therefore that an HTTP GET of a just-returned URI reaches the service that minted it and returns 200 with the stored content — the vocabulary's phrase list, the filter's word list, or the transcript JSON document.

Emulator: the URI is constructed from the real AWS S3 endpoint and the real service-managed bucket name: services/service-com-amazonaws-transcribe-transcribe/src/state.rs:4012 vocabulary_download_uri formats "https://s3.{region}.amazonaws.com/{bucket}/{key}", with the bucket coming from state.rs:4024 service_managed_bucket = "aws-transcribe-{region}-prod"; output_uri's service-managed branch builds the transcript location the same way. So GetVocabulary/GetVocabularyFilter answer DownloadUri = https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/vocabulary|vocabulary-filter/<name> and GetTranscriptionJob answers Transcript.TranscriptFileUri = https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/<job-name>.json. The host in those URIs is not one of the emulator's own hosts, so a customer of the emulator cannot reach it: the fetch is refused with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>"). The content itself IS stored inside the emulator — store_vocabulary_content (state.rs:4138) and store_job_outputs write the object through the in-process S3 callback into the same bucket/key — so only the ANNOUNCED address diverges, never the stored bytes.

Why: the whole Transcribe data plane is unreachable through the emulator, and worse, the address it publishes points at real AWS. A client that follows the URI the service just handed it either leaves the emulator entirely (an egress attempt against s3.<region>.amazonaws.com, where the account holds no such bucket) or is refused outright. A vocabulary's phrases, a filter's words and every transcript are unobtainable: a job reports COMPLETED while its output cannot be downloaded, and no round-trip verification (did my words persist? did my update take effect? did the race winner's phrase survive?) is possible. The failure is not the modeled AccessDenied-on-expiry either, so clients cannot recover by re-issuing GetTranscriptionJob.

Fix: the service-managed URI minting in the transcribe crate — vocabulary_download_uri and service_managed_bucket in state.rs, and output_uri's service-managed branch that feeds Transcript.TranscriptFileUri / RedactedTranscriptFileUri and SubtitlesOutput.SubtitleFileUris — must publish the emulator's own externally-reachable endpoint (the host/port the request arrived on, or a per-resource listener) for the same bucket/key the write addresses, so the reported location and the served object stay in agreement and a GET issued right after the URI is returned yields 200 and the stored content.

Traces: the seven check ids in trace_ids; each fails at its first fetch of a URI the emulator itself just returned. Two open issues describe the same symptom family with a different failure key and do not carry these replay keys: 20261029T184620Z-transcribe-every-downloaduri-transcriptfileuri-transcribe-hands-back-answers-403.md and 20261103T162152Z-transcribe-vocabulary-downloaduri-returned-under-concurrent-createvocabulary-answers-403.md both record the URI being FETCHED and answered HTTP 403, whereas the emulator now publishes a host outside itself so the fetch never reaches a server at all.

## Evidence
Expected: GET of a just-returned DownloadUri / TranscriptFileUri returns 200 and the stored content (the declared Words for a filter, the declared Phrases for a vocabulary, a transcript JSON whose jobName equals the job).
Actual: simparity.published.PublishedUrlRejected raised at the first fetch, because the host announced by the emulator is not one it serves. Verbatim form (host/port and generated names vary per run; the pattern is the requirement):
  "endpoint_url was given https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/vocabulary/<vocabulary-name> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"
Observed instances:
  - transcribe.vocabulary-filter-content-roundtrip-via-download-uri — .../aws-transcribe-us-west-2-prod/000000000000/vocabulary-filter/<filter-name> (GetVocabularyFilter.DownloadUri)
  - transcribe.update-vocabulary-filter-words-reflected-in-download — .../vocabulary-filter/<filter-name>
  - transcribe.update-vocabulary-resets-state-and-content — .../vocabulary/<vocabulary-name> (GetVocabulary.DownloadUri)
  - transcribe.create-vocabulary-duplicate-name-conflict — .../vocabulary/<vocabulary-name>, after the ConflictException assertions had passed
  - transcribe.concurrent-create-vocabulary-same-name-one-wins — .../vocabulary/<vocabulary-name>, after exactly-one-winner/ConflictException assertions had passed
  - transcribe.transcription-job-lifecycle-completes — .../aws-transcribe-us-west-2-prod/000000000000/<job-name>.json (Transcript.TranscriptFileUri)
  - transcribe.native-speech-to-text-round-trip-via-polly — .../<job-name>.json (Transcript.TranscriptFileUri)
Source of the divergence: services/service-com-amazonaws-transcribe-transcribe/src/state.rs:4012-4018 (vocabulary_download_uri -> "https://s3.{region}.amazonaws.com/{bucket}/{key}") and state.rs:4024-4026 (service_managed_bucket -> "aws-transcribe-{region}-prod"); the object is written to that same bucket/key in-process at state.rs:4138-4197, so the bytes exist but the announced host does not belong to the emulator.

## Steps to Reproduce
1. CreateVocabularyFilter(VocabularyFilterName=F, LanguageCode=en-US, Words=["dang","heck"]), then GetVocabularyFilter(F) and read DownloadUri -> it is https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/vocabulary-filter/F, a host the emulator does not serve. HTTP GET it: it does not resolve to the emulator (and against real AWS the bucket is not the caller's), instead of 200 + the word list.
2. CreateVocabulary(VocabularyName=V, LanguageCode=en-US, Phrases=["Alderaan"]), poll GetVocabulary until VocabularyState=READY, read DownloadUri -> same host; GET it -> unreachable instead of 200 + the phrase list.
3. StartTranscriptionJob with no OutputBucketName, poll GetTranscriptionJob until COMPLETED, read Transcript.TranscriptFileUri -> https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/<job>.json; GET it -> unreachable instead of 200 + the transcript JSON document.
4. Replay: run_behavioral_suite and re-run the seven checks in trace_ids.
