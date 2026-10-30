Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-call-analytics-category-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-call-analytics-category-lifecycle transcribe-not-found-error-paths
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|GetCallAnalyticsCategory|4xx->4xx|$.Message
Issue-Key: transcribe|UpdateCallAnalyticsCategory|4xx->4xx|$.Message
Issue-Key: transcribe|CreateCallAnalyticsCategory|4xx->4xx|$.Message
Issue-Key: transcribe|CreateCallAnalyticsCategory|4xx->4xx|$.__type
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Call Analytics category refusals carry emulator-authored codes and messages instead of AWS's

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallAnalyticsCategory

## Description
Every negative path of the Call Analytics category API answers with the right HTTP status but an error payload the emulator wrote itself: the not-found message is reworded on both operations that can raise it, and the duplicate-name refusal additionally carries the WRONG error code. 3 causes.

Smithy: GetCallAnalyticsCategory models BadRequestException (400, Message -> FailureReason), CreateCallAnalyticsCategory and UpdateCallAnalyticsCategory model both BadRequestException (400) and ConflictException (409). The model therefore does not pick which code a duplicate name gets, and it never states the message text, so the exact code/text pairing per class is recorded behavior only. For Transcribe the message is load-bearing: BadRequestException is reused for not-found, non-terminal-state and validation failures, so the text is the only thing that distinguishes them.

Cause 1 of 3: GetCallAnalyticsCategory on a category name that does not exist.
AWS: BadRequestException, HTTP 400, with the entirely fixed text 'The category that you’ve specified couldn’t be found. Check the category name and try your request again.' — note both apostrophes are U+2019 (RIGHT SINGLE QUOTATION MARK), not ASCII "'", and the requested CategoryName does NOT appear anywhere in the message.
Emulator: BadRequestException, HTTP 400, with "The requested category couldn't be found. Check the category name and try your request again." — different leading clause and ASCII apostrophe.
Why: a client or test keyed on AWS's not-found text does not recognise the refusal and cannot tell a missing category apart from the other 400s this operation returns, since the code is identical for all of them.
Fix: the not-found message constant used by GetCallAnalyticsCategory's lookup path in the transcribe handler.
Traces: transcribe-call-analytics-category-lifecycle#task#1 (the same cluster also covers transcribe-call-analytics-category-lifecycle#teardown#9)

Cause 2 of 3: UpdateCallAnalyticsCategory on a category name that does not exist.
AWS: the same BadRequestException/400 and the same fixed text as cause 1 — AWS uses one category-not-found message for the whole family; no part of it comes from the request.
Emulator: the same reworded "The requested category couldn't be found. ..." text.
Why: same as cause 1, on the update path; the cluster shows it on a dedicated not-found fixture too.
Fix: the not-found message constant used by UpdateCallAnalyticsCategory's lookup path (share one constant with cause 1).
Traces: transcribe-call-analytics-category-lifecycle#teardown#10 (the same cluster also covers transcribe-not-found-error-paths#task#3)

Cause 3 of 3: CreateCallAnalyticsCategory with a CategoryName that already exists — wrong error CODE as well as wrong text.
AWS: BadRequestException with __type 'BadRequestException' at HTTP 400, message 'The category name that you’ve requested already exists. Change the category name and try your request again.' (fixed text, U+2019 apostrophe, the conflicting CategoryName is not interpolated). AWS does NOT use ConflictException for this path even though the operation models it.
Emulator: __type 'ConflictException' with message 'The requested category name already exists. Use a different category name.' — served at HTTP 400, which is not even ConflictException's modeled 409, so the payload is internally inconsistent with the model as well as with AWS.
Why: this is the most damaging of the three. SDK error-handling branches on __type, so a client that catches BadRequestException (as AWS requires for this path) sees an unmodeled/unexpected ConflictException escape, while a client that catches ConflictException gets a 400 where the model promises 409. Retry and name-reallocation logic built on the recorded code never fires.
Fix: CreateCallAnalyticsCategory's duplicate-name guard in the transcribe handler — raise the BadRequestException carrier with the fixed duplicate-name text instead of ConflictException.
Traces: transcribe-call-analytics-category-lifecycle#task#3

Related but not covering: the open behavioral issue 20261029T184620Z-transcribe-rejection-conflict-error-messages-are-emulator-authored reports the same class of defect (emulator-authored refusal prose) for six OTHER Transcribe paths (CreateVocabulary, CreateVocabularyFilter, StartTranscriptionJob, DeleteVocabulary, DeleteTranscriptionJob). None of its causes is a Call Analytics category operation and it carries no wire replay keys, so the three causes here are net-new.

## Evidence
Cause 1 (task#1, GetCallAnalyticsCategory, status 400 expected and actual): $.Message value_mismatch — expected 'The category that you’ve specified couldn’t be found. Check the category name and try your request again.', got "The requested category couldn't be found. Check the category name and try your request again.". Fixed text: the entire message. Variable parts: none — the emulator and AWS both interpolate nothing, the words simply differ.
Cause 2 (teardown#10, UpdateCallAnalyticsCategory, status 400 expected and actual): identical expected/actual pair as cause 1.
Cause 3 (task#3, CreateCallAnalyticsCategory, status 400 expected and actual): $.__type value_mismatch — expected 'BadRequestException', got 'ConflictException'; $.Message value_mismatch — expected 'The category name that you’ve requested already exists. Change the category name and try your request again.', got 'The requested category name already exists. Use a different category name.'. Fixed text: the whole expected message; variable parts: none (the CategoryName is not part of it).

## Steps to Reproduce
1. GetCallAnalyticsCategory(CategoryName=<a name never created>) and read __type/Message of the 400.
2. UpdateCallAnalyticsCategory(CategoryName=<a name never created>, Rules=[{TranscriptFilter:{TranscriptFilterType:'EXACT', Targets:['goodbye']}}]) and read Message of the 400.
3. CreateCallAnalyticsCategory(CategoryName=N, Rules=[...]) once successfully, then call it again with the SAME N; read __type and Message of the refusal — AWS: BadRequestException + duplicate-name text at 400; emulator: ConflictException + its own text at 400.
Replay: transcribe-call-analytics-category-lifecycle steps task#1, teardown#9, teardown#10 and task#3, plus transcribe-not-found-error-paths step task#3.
