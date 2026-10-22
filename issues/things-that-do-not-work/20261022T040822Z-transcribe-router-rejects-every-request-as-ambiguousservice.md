Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-call-analytics-category-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-call-analytics-category-lifecycle transcribe-not-found-error-paths transcribe-call-analytics-job-workflow transcribe-language-model-lifecycle transcribe-medical-scribe-job-workflow transcribe-medical-transcription-job-workflow transcribe-medical-vocabulary-lifecycle transcribe-vocabulary-lifecycle transcribe-tag-resource-list-untag-roundtrip transcribe-transcription-job-workflow transcribe-vocabulary-filter-lifecycle
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|GetCallAnalyticsCategory|4xx->5xx|$.Message
Issue-Key: transcribe|GetCallAnalyticsCategory|4xx->5xx|$.__type
Issue-Key: transcribe|GetCallAnalyticsCategory|4xx->5xx|status
Issue-Key: transcribe|CreateCallAnalyticsCategory|2xx->5xx|$.CategoryProperties
Issue-Key: transcribe|CreateCallAnalyticsCategory|2xx->5xx|status
Issue-Key: transcribe|CreateCallAnalyticsCategory|4xx->5xx|$.Message
Issue-Key: transcribe|CreateCallAnalyticsCategory|4xx->5xx|$.__type
Issue-Key: transcribe|CreateCallAnalyticsCategory|4xx->5xx|status
Issue-Key: transcribe|GetCallAnalyticsCategory|2xx->5xx|$.CategoryProperties
Issue-Key: transcribe|GetCallAnalyticsCategory|2xx->5xx|status
Issue-Key: transcribe|UpdateCallAnalyticsCategory|2xx->5xx|$.CategoryProperties
Issue-Key: transcribe|UpdateCallAnalyticsCategory|2xx->5xx|status
Issue-Key: transcribe|ListCallAnalyticsCategories|2xx->5xx|$.Categories
Issue-Key: transcribe|ListCallAnalyticsCategories|2xx->5xx|status
Issue-Key: transcribe|DeleteCallAnalyticsCategory|2xx->5xx|status
Issue-Key: transcribe|UpdateCallAnalyticsCategory|4xx->5xx|$.Message
Issue-Key: transcribe|UpdateCallAnalyticsCategory|4xx->5xx|$.__type
Issue-Key: transcribe|UpdateCallAnalyticsCategory|4xx->5xx|status
Issue-Key: transcribe|GetCallAnalyticsJob|4xx->5xx|$.Message
Issue-Key: transcribe|GetCallAnalyticsJob|4xx->5xx|$.__type
Issue-Key: transcribe|GetCallAnalyticsJob|4xx->5xx|status
Issue-Key: transcribe|StartCallAnalyticsJob|4xx->5xx|$.Message
Issue-Key: transcribe|StartCallAnalyticsJob|4xx->5xx|$.__type
Issue-Key: transcribe|StartCallAnalyticsJob|4xx->5xx|status
Issue-Key: transcribe|ListCallAnalyticsJobs|2xx->5xx|$.CallAnalyticsJobSummaries
Issue-Key: transcribe|ListCallAnalyticsJobs|2xx->5xx|status
Issue-Key: transcribe|DeleteCallAnalyticsJob|4xx->5xx|$.Message
Issue-Key: transcribe|DeleteCallAnalyticsJob|4xx->5xx|$.__type
Issue-Key: transcribe|DeleteCallAnalyticsJob|4xx->5xx|status
Issue-Key: transcribe|DescribeLanguageModel|4xx->5xx|$.Message
Issue-Key: transcribe|DescribeLanguageModel|4xx->5xx|$.__type
Issue-Key: transcribe|DescribeLanguageModel|4xx->5xx|status
Issue-Key: transcribe|CreateLanguageModel|4xx->5xx|$.Message
Issue-Key: transcribe|CreateLanguageModel|4xx->5xx|$.__type
Issue-Key: transcribe|CreateLanguageModel|4xx->5xx|status
Issue-Key: transcribe|ListLanguageModels|2xx->5xx|$.Models
Issue-Key: transcribe|ListLanguageModels|2xx->5xx|status
Issue-Key: transcribe|GetMedicalScribeJob|4xx->5xx|$.Message
Issue-Key: transcribe|GetMedicalScribeJob|4xx->5xx|$.__type
Issue-Key: transcribe|GetMedicalScribeJob|4xx->5xx|status
Issue-Key: transcribe|StartMedicalScribeJob|4xx->5xx|$.Message
Issue-Key: transcribe|StartMedicalScribeJob|4xx->5xx|$.__type
Issue-Key: transcribe|StartMedicalScribeJob|4xx->5xx|status
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->5xx|$.Message
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->5xx|$.__type
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->5xx|status
Issue-Key: transcribe|DeleteMedicalScribeJob|4xx->5xx|$.Message
Issue-Key: transcribe|DeleteMedicalScribeJob|4xx->5xx|$.__type
Issue-Key: transcribe|DeleteMedicalScribeJob|4xx->5xx|status
Issue-Key: transcribe|GetMedicalTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|GetMedicalTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|GetMedicalTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|StartMedicalTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|StartMedicalTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|StartMedicalTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|ListMedicalTranscriptionJobs|2xx->5xx|$.MedicalTranscriptionJobSummaries
Issue-Key: transcribe|ListMedicalTranscriptionJobs|2xx->5xx|status
Issue-Key: transcribe|DeleteMedicalTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|DeleteMedicalTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|DeleteMedicalTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|GetMedicalVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|GetMedicalVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|GetMedicalVocabulary|4xx->5xx|status
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|CreateMedicalVocabulary|4xx->5xx|status
Issue-Key: transcribe|ListMedicalVocabularies|2xx->5xx|$.Vocabularies
Issue-Key: transcribe|ListMedicalVocabularies|2xx->5xx|status
Issue-Key: transcribe|UpdateMedicalVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|UpdateMedicalVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|UpdateMedicalVocabulary|4xx->5xx|status
Issue-Key: transcribe|DeleteMedicalVocabulary|2xx->5xx|$
Issue-Key: transcribe|DeleteMedicalVocabulary|2xx->5xx|status
Issue-Key: transcribe|UpdateVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|UpdateVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|UpdateVocabulary|4xx->5xx|status
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->5xx|$.Message
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->5xx|$.__type
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->5xx|status
Issue-Key: transcribe|ListTagsForResource|4xx->5xx|$.Message
Issue-Key: transcribe|ListTagsForResource|4xx->5xx|$.__type
Issue-Key: transcribe|ListTagsForResource|4xx->5xx|status
Issue-Key: transcribe|TagResource|4xx->5xx|$.Message
Issue-Key: transcribe|TagResource|4xx->5xx|$.__type
Issue-Key: transcribe|TagResource|4xx->5xx|status
Issue-Key: transcribe|UntagResource|4xx->5xx|$.Message
Issue-Key: transcribe|UntagResource|4xx->5xx|$.__type
Issue-Key: transcribe|UntagResource|4xx->5xx|status
Issue-Key: transcribe|CreateVocabulary|2xx->5xx|status
Issue-Key: transcribe|GetTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|GetTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|GetTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|StartTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|StartTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|StartTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|ListTranscriptionJobs|2xx->5xx|$.TranscriptionJobSummaries
Issue-Key: transcribe|ListTranscriptionJobs|2xx->5xx|status
Issue-Key: transcribe|DeleteTranscriptionJob|4xx->5xx|$.Message
Issue-Key: transcribe|DeleteTranscriptionJob|4xx->5xx|$.__type
Issue-Key: transcribe|DeleteTranscriptionJob|4xx->5xx|status
Issue-Key: transcribe|GetVocabularyFilter|4xx->5xx|$.Message
Issue-Key: transcribe|GetVocabularyFilter|4xx->5xx|$.__type
Issue-Key: transcribe|GetVocabularyFilter|4xx->5xx|status
Issue-Key: transcribe|CreateVocabularyFilter|2xx->5xx|$.LanguageCode
Issue-Key: transcribe|CreateVocabularyFilter|2xx->5xx|$.VocabularyFilterName
Issue-Key: transcribe|CreateVocabularyFilter|2xx->5xx|status
Issue-Key: transcribe|CreateVocabularyFilter|4xx->5xx|$.Message
Issue-Key: transcribe|CreateVocabularyFilter|4xx->5xx|$.__type
Issue-Key: transcribe|CreateVocabularyFilter|4xx->5xx|status
Issue-Key: transcribe|GetVocabularyFilter|2xx->5xx|$.DownloadUri
Issue-Key: transcribe|GetVocabularyFilter|2xx->5xx|$.LanguageCode
Issue-Key: transcribe|GetVocabularyFilter|2xx->5xx|$.LastModifiedTime
Issue-Key: transcribe|GetVocabularyFilter|2xx->5xx|$.VocabularyFilterName
Issue-Key: transcribe|GetVocabularyFilter|2xx->5xx|status
Issue-Key: transcribe|UpdateVocabularyFilter|2xx->5xx|$.LanguageCode
Issue-Key: transcribe|UpdateVocabularyFilter|2xx->5xx|$.LastModifiedTime
Issue-Key: transcribe|UpdateVocabularyFilter|2xx->5xx|$.VocabularyFilterName
Issue-Key: transcribe|UpdateVocabularyFilter|2xx->5xx|status
Issue-Key: transcribe|ListVocabularyFilters|2xx->5xx|$.VocabularyFilters
Issue-Key: transcribe|ListVocabularyFilters|2xx->5xx|status
Issue-Key: transcribe|DeleteVocabularyFilter|2xx->5xx|status
Issue-Key: transcribe|GetVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|GetVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|GetVocabulary|4xx->5xx|status
Issue-Key: transcribe|CreateVocabulary|4xx->5xx|$.Message
Issue-Key: transcribe|CreateVocabulary|4xx->5xx|$.__type
Issue-Key: transcribe|CreateVocabulary|4xx->5xx|status
Issue-Key: transcribe|ListVocabularies|2xx->5xx|$.Vocabularies
Issue-Key: transcribe|ListVocabularies|2xx->5xx|status
Issue-Key: transcribe|DeleteVocabulary|2xx->5xx|$
Issue-Key: transcribe|DeleteVocabulary|2xx->5xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# Transcribe router rejects every request as AmbiguousService

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallAnalyticsCategory

## Description
Cause 1 of 1: the emulator's service-dispatch layer cannot tell the two bootstrapped services that share the sigv4 signing name 'transcribe' apart, so it refuses to serve EITHER and answers 500 with __type 'AmbiguousService' for every single Amazon Transcribe request. All 49 divergence clusters in this corpus (56 of 62 replayed steps, score 0.0%, 0 steps passed) are this one fault; no Transcribe operation is reachable at all. Representative fixture/step: transcribe-call-analytics-category-lifecycle#task#1 (GetCallAnalyticsCategory). Every other cluster's representative binding is listed in trace_ids and must be re-replayed before this is closed.

AWS: Amazon Transcribe (Smithy: services/service-com-amazonaws-transcribe-transcribe, sdkId 'Transcribe', apiVersion 2017-10-26, protocol awsJson1_1, endpointPrefix 'transcribe', sigv4Name 'transcribe') serves all 44 of its bound operations. Requests dispatch by X-Amz-Target ('Transcribe.<Operation>'; the modeled @http/@httpLabel bindings are inert under awsJson1_1) and each operation returns either its modeled 200 output shape or one of its modeled client errors — e.g. GetCallAnalyticsCategory.errors = BadRequestException(400), NotFoundException(404), LimitExceededException(429); CreateVocabularyFilter.errors = BadRequestException(400), ConflictException(409); ListVocabularies.errors = BadRequestException(400). The recordings show the real service returning 400 with __type 'BadRequestException' on the not-found/invalid-input paths, 400 with __type 'ConflictException' on duplicate CreateVocabularyFilter, and 200 with the modeled output body on the success paths. 'AmbiguousService' is not a modeled error of this service and 500 is not a modeled client-error status for any of these operations. Note that the second service sharing the signing name, 'Transcribe Streaming' (sdkId 'Transcribe Streaming', protocol restJson1, endpointPrefix 'transcribestreaming', sigv4Name 'transcribe'), is a DIFFERENT service with only 5 operations; on AWS the two are distinguished by endpoint host/endpointPrefix and by protocol, not by signing name.

Emulator: every replayed step returns HTTP 500 with body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'transcribe': transcribe, transcribe; the request is rejected rather than served by an arbitrary one"}. Replay reports status 500 against every expected status, __type 'AmbiguousService' where 'BadRequestException' or 'ConflictException' was expected, and the whole success payload absent ($.CategoryProperties, $.Categories, $.Vocabularies, $.VocabularyFilters, $.Models, $.TranscriptionJobSummaries, $.MedicalTranscriptionJobSummaries, $.CallAnalyticsJobSummaries, $.VocabularyFilterName, $.LanguageCode, $.LastModifiedTime, $.DownloadUri all reported missing). The router resolves a request to a service by sigv4 signing name alone; because both registered services declare sigv4Name 'transcribe' it finds two candidates and fails closed instead of discriminating.

Why: a client gets a 500 server-fault for every Transcribe call, including calls that should succeed and calls that should be a well-formed 4xx. SDK retry logic treats 500 as retryable, so clients retry a request that can never succeed and then surface an unmodeled error; code that catches BadRequestException/ConflictException/NotFoundException never sees them, so not-found and already-exists handling, pagination over empty lists, and every create/get/update/delete lifecycle are all broken. In effect the Transcribe emulator is bootstrapped but unusable.

Fix: the shared request-dispatch/service-resolution layer that maps an inbound request to an emulated service. Resolution must not key on the sigv4 signing name alone: disambiguate the two 'transcribe'-signed services by endpointPrefix/Host ('transcribe' vs 'transcribestreaming'), by protocol shape (awsJson1_1 X-Amz-Target 'Transcribe.<Op>' for Amazon Transcribe vs restJson1 method+URI for Transcribe Streaming), or by which of the two actually binds the requested operation — and only reject as ambiguous when no candidate claims the request. Once dispatch reaches the Transcribe handlers, re-replay the whole corpus: any residual per-operation mismatch is masked today and will need its own issue.

Traces: representative transcribe-call-analytics-category-lifecycle#task#1; all 49 cluster representatives in trace_ids, spanning fixtures transcribe-call-analytics-category-lifecycle, transcribe-call-analytics-job-workflow, transcribe-language-model-lifecycle, transcribe-medical-scribe-job-workflow, transcribe-medical-transcription-job-workflow, transcribe-medical-vocabulary-lifecycle, transcribe-not-found-error-paths, transcribe-tag-resource-list-untag-roundtrip, transcribe-transcription-job-workflow, transcribe-vocabulary-filter-lifecycle and transcribe-vocabulary-lifecycle.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 62, steps_passed 0, steps_mismatched 56, steps_blocked 6, score_pct 0.0, divergence_count 49, state_reset_failures 0.

Expected vs actual, by shape of divergence:
- Error paths (expected 400 / __type 'BadRequestException' + Message): got 500 / __type 'AmbiguousService', Message absent. e.g. GetCallAnalyticsCategory (transcribe-call-analytics-category-lifecycle#task#1): status expected 400, got 500; $.__type expected 'BadRequestException', got 'AmbiguousService'; $.Message missing (recorded example: 'The category that you\u2019ve specified couldn\u2019t be found. Check the category name and try your request again.'). Same shape for GetCallAnalyticsJob, StartCallAnalyticsJob, DeleteCallAnalyticsJob, DescribeLanguageModel, CreateLanguageModel, GetMedicalScribeJob, StartMedicalScribeJob, ListMedicalScribeJobs, DeleteMedicalScribeJob, GetMedicalTranscriptionJob, StartMedicalTranscriptionJob, DeleteMedicalTranscriptionJob, GetMedicalVocabulary, CreateMedicalVocabulary, UpdateMedicalVocabulary, UpdateVocabulary, UpdateVocabularyFilter, UpdateCallAnalyticsCategory, CreateCallAnalyticsCategory, ListTagsForResource, TagResource, UntagResource, GetTranscriptionJob, StartTranscriptionJob, DeleteTranscriptionJob, GetVocabularyFilter, GetVocabulary, CreateVocabulary.
- Conflict path: CreateVocabularyFilter (transcribe-vocabulary-filter-lifecycle#task#3): status expected 400, got 500; $.__type expected 'ConflictException', got 'AmbiguousService'.
- Success paths (expected 200 + modeled body): got 500 with the AmbiguousService body and no output members. e.g. CreateCallAnalyticsCategory#task#2 ($.CategoryProperties missing), ListCallAnalyticsCategories#task#7 ($.Categories missing), ListVocabularies (transcribe-vocabulary-lifecycle#task#3, $.Vocabularies expected [], missing), ListVocabularyFilters#task#7 ($.VocabularyFilters missing), ListLanguageModels ($.Models missing), ListTranscriptionJobs ($.TranscriptionJobSummaries missing), ListMedicalTranscriptionJobs, ListCallAnalyticsJobs, ListMedicalVocabularies, CreateVocabularyFilter#task#2 ($.VocabularyFilterName, $.LanguageCode missing), GetVocabularyFilter#task#4 ($.VocabularyFilterName, $.LanguageCode, $.LastModifiedTime, $.DownloadUri missing), UpdateVocabularyFilter#task#5, DeleteCallAnalyticsCategory#teardown#8, DeleteVocabularyFilter#teardown#8, CreateVocabulary (transcribe-tag-resource-list-untag-roundtrip#setup#1).
- Empty-body success paths show the fault verbatim: DeleteVocabulary (transcribe-vocabulary-lifecycle#task#5) and DeleteMedicalVocabulary (transcribe-medical-vocabulary-lifecycle#task#5): expected 200 with body {}, got 500 with {'__type': 'AmbiguousService', 'message': "2 emulated services all claim requests for 'transcribe': transcribe, transcribe; the request is rejected rather than served by an arbitrary one"}.

Model evidence for the collision: services/service-com-amazonaws-transcribe-transcribe -> sdkId 'Transcribe', protocol awsJson1_1, endpointPrefix 'transcribe', sigv4Name 'transcribe', 44 operations. services/service-com-amazonaws-transcribestreaming-transcribe -> sdkId 'Transcribe Streaming', protocol restJson1, endpointPrefix 'transcribestreaming', sigv4Name 'transcribe', 5 operations. Both are reported bootstrapped by get_bootstrap_status. Recorded Message strings above are examples only; the requirements are the status codes and the __type error codes.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (admin health answers 200; data listener accepts).
2. Replay the simparity-wire-aws-v1 corpus: every one of the 11 transcribe fixtures diverges on every step (0/62 passed, 56 mismatched, 6 blocked).
3. Minimal repro of the shared cause - send any awsJson1_1 Transcribe request to the data listener signed for service 'transcribe', e.g. POST / with headers 'X-Amz-Target: Transcribe.ListVocabularies', 'Content-Type: application/x-amz-json-1.1' and body {}. Expected: 200 with {"Vocabularies": []}. Actual: 500 with {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'transcribe': transcribe, transcribe; ..."}.
4. Repro an error path the same way: 'X-Amz-Target: Transcribe.GetVocabulary' with body {"VocabularyName": "does-not-exist"}. Expected: 400 with __type 'BadRequestException'. Actual: 500 with __type 'AmbiguousService'.
5. Confirm the collision in the models: both services/service-com-amazonaws-transcribe-transcribe and services/service-com-amazonaws-transcribestreaming-transcribe declare sigv4Name 'transcribe' while declaring different endpointPrefix values ('transcribe' vs 'transcribestreaming') and different protocols (awsJson1_1 vs restJson1).
6. After fixing dispatch, re-replay the full corpus and confirm each fixture/step named in Issue-Trace/Issue-Step and trace_ids.
