Issue-Source: trace-sweep-divergence
Issue-Trace: textract-adapter-and-resource-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-adapter-and-resource-error-paths textract-async-start-and-get-error-paths textract-sync-analyze-error-paths
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|TagResource|4xx->4xx|$.Message
Issue-Key: textract|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: textract|UntagResource|4xx->4xx|$.Message
Issue-Key: textract|GetDocumentTextDetection|4xx->4xx|$.Message
Issue-Key: textract|GetDocumentAnalysis|4xx->4xx|$.Message
Issue-Key: textract|AnalyzeDocument|4xx->4xx|$.Message
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Textract client-error refusals return non-AWS Message text on six operations

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Six textract refusal paths DO refuse the request and DO return the right HTTP status and the right modeled error code, but the `Message` they carry is the emulator's own prose rather than the fixed short string Textract sends for that error class. An HTTP status, an error code AND an error message are all requirements, so each of these is a divergence a client observes. Two shared defects produce them: the adapter-not-found lookup appends a trailing period AWS does not send, and three validation paths emit a self-written explanation (one of which interpolates the request's JobId) where AWS sends constant text. This issue bundles 6 distinct root causes, all in the textract crate; a fixer must close ALL SIX before submitting.

Cause 1 of 6 - TagResource on an unknown adapter ARN returns 'Adapter Id not found.' instead of 'Adapter Id not found'.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): TagResource input {ResourceARN required @length 1..1011, Tags required}; ResourceNotFoundException is a modeled client error at HTTP 400, documented "Returned when an operation tried to access a nonexistent resource." For a well-formed adapter ARN that names no existing adapter, AWS answers HTTP 400 with that code and message "Adapter Id not found" - the message is wholly FIXED text, with NO trailing period and with no part interpolated from the request (it does not name the ARN or the adapter id).
Emulator: HTTP 400 and the correct code, but Message 'Adapter Id not found.' - the same sentence with a trailing '.' appended. The period is fixed emulator text, not a per-run value.
Why: the message is part of the refusal a client logs, asserts on and surfaces to an operator. A client or test matching AWS's exact text does not recognize the emulator's, so a correctly-detected missing adapter reads as an unknown failure; equality comparisons on Error.Message fail on every call.
Fix: the shared adapter-lookup refusal in the textract crate's state/adapter-resolution helper that the resource-tagging handlers call - construct ResourceNotFoundException with exactly "Adapter Id not found" (no trailing punctuation). Because Causes 1-3 come from the same helper, one change should close all three; each must still be re-replayed.
Traces: textract-adapter-and-resource-error-paths#task#6.

Cause 2 of 6 - ListTagsForResource on an unknown adapter ARN carries the same punctuated message.
AWS: Smithy: ListTagsForResource input {ResourceARN required @length 1..1011}, output {Tags}; ResourceNotFoundException modeled as a client error at HTTP 400. An ARN naming no existing adapter is refused HTTP 400 / ResourceNotFoundException with the fixed message "Adapter Id not found".
Emulator: HTTP 400 and the correct code, Message 'Adapter Id not found.'.
Why: same as Cause 1 - a client reading tags of an adapter it believes exists cannot match the documented refusal text, so its error branch falls through to a generic handler.
Fix: the same adapter-lookup refusal reached from ListTagsForResource's handler in the textract crate.
Traces: textract-adapter-and-resource-error-paths#task#7.

Cause 3 of 6 - UntagResource on an unknown adapter ARN carries the same punctuated message.
AWS: Smithy: UntagResource input {ResourceARN required @length 1..1011, TagKeys required}; ResourceNotFoundException modeled as a client error at HTTP 400. An ARN naming no existing adapter is refused HTTP 400 / ResourceNotFoundException with the fixed message "Adapter Id not found".
Emulator: HTTP 400 and the correct code, Message 'Adapter Id not found.'.
Why: same as Cause 1.
Fix: the same adapter-lookup refusal reached from UntagResource's handler in the textract crate.
Traces: textract-adapter-and-resource-error-paths#task#8.

Cause 4 of 6 - GetDocumentTextDetection's unknown-JobId refusal replaces AWS's fixed message with generated prose that interpolates the submitted JobId.
AWS: Smithy: GetDocumentTextDetection input {JobId required @length 1..64 @pattern ^[a-zA-Z0-9-_]+$, MaxResults, NextToken}; InvalidJobIdException is the modeled client error at HTTP 400, documented "An invalid job identifier was passed to an asynchronous analysis operation." A syntactically valid JobId the service never issued is refused HTTP 400 / InvalidJobIdException with message "Request has invalid Job Id" - entirely fixed text that does NOT contain the submitted job id or any other request value.
Emulator: HTTP 400 and the correct code, but Message '<the submitted JobId> is not a text detection job started by this service' - the leading token is the JobId copied from the request and the remainder (' is not a text detection job started by this service') is fixed emulator text. AWS interpolates nothing here.
Why: the client's error branch is keyed on code plus message. AWS's message is constant and id-free, so a client that asserts on or aggregates the message sees a different string on every call against the emulator, and the emulator additionally echoes caller-supplied input back into the error surface, which AWS does not do for this error.
Fix: the unknown-job refusal in GetDocumentTextDetection's handler (and the shared job-lookup it uses) - emit InvalidJobIdException with the fixed message "Request has invalid Job Id" and stop interpolating the JobId.
Traces: textract-async-start-and-get-error-paths#task#2.

Cause 5 of 6 - GetDocumentAnalysis's unknown-JobId refusal has the same defect with a differently worded sentence.
AWS: Smithy: GetDocumentAnalysis input {JobId required @length 1..64 @pattern ^[a-zA-Z0-9-_]+$, MaxResults, NextToken}; InvalidJobIdException modeled as a client error at HTTP 400. A JobId the service never issued is refused HTTP 400 / InvalidJobIdException with the fixed message "Request has invalid Job Id" - the SAME constant text as GetDocumentTextDetection; AWS does not vary it by job kind.
Emulator: HTTP 400 and the correct code, Message '<the submitted JobId> is not a document analysis job started by this service' - again the request's JobId followed by fixed emulator text, and a second distinct sentence where AWS has one.
Why: as Cause 4, plus a client that handles both Get* operations cannot use a single message expectation because the emulator differentiates text AWS keeps identical.
Fix: the unknown-job refusal in GetDocumentAnalysis's handler; it must agree with the GetDocumentTextDetection path on the single fixed message "Request has invalid Job Id".
Traces: textract-async-start-and-get-error-paths#task#4.

Cause 6 of 6 - AnalyzeDocument's empty-FeatureTypes refusal reports the emulator's own constraint sentence instead of AWS's message.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}; InvalidParameterException is the modeled client error at HTTP 400, documented "An input parameter violated a constraint... Validate your parameter before calling the API operation again." A request whose FeatureTypes list is present but empty is refused HTTP 400 / InvalidParameterException with message "Request has invalid parameters" - fixed text that names neither the offending member nor the rule it broke.
Emulator: HTTP 400 and the correct code, but Message 'FeatureTypes must contain at least one FeatureType' - a self-written constraint description. All of it is fixed emulator text; none of it appears in AWS's message.
Why: a client matching the documented refusal text does not recognize it, and code written against the emulator's member-naming message will not match AWS, where the generic 'Request has invalid parameters' string is shared across Textract's parameter refusals.
Fix: AnalyzeDocument's input validation in its operation handler - refuse an empty/missing FeatureTypes with InvalidParameterException (HTTP 400) and message "Request has invalid parameters", rather than the hand-written constraint sentence.
Traces: textract-sync-analyze-error-paths#task#2.

Related open issues, none of which covers these six wire steps: 20261029T184456Z-textract-refusals-use-the-wrong-error-code-and.md is a BEHAVIORAL issue whose four causes carry only behavioral check ids (textract.*) - its Cause 4 concerns the same GetDocumentTextDetection JobId wording but carries no wire replay key, so nothing re-replays these fixtures, and it does not mention GetDocumentAnalysis as a cause of its own, the adapter-not-found punctuation, or the empty-FeatureTypes message. 20261023T210700Z-textract-accepts-invalid-requests-with-http-200-instead.md owns only the textract steps that answer 200 where AWS answers 400 (it owns this corpus's ListAdapters and DetectDocumentText-unsupported-format steps); the six causes here are refusals that DO happen, with the right status and code and the wrong message.

## Evidence
Trace set simparity-wire-aws-v1 replayed against the emulator booted from this checkout: emulator_up true, bootstrap_signal false, 6 fixtures replayed, 38 steps total, 19 passed, 19 mismatched, 0 errored, 0 blocked, score 50.0%, 10 divergence clusters. All clusters are service 'textract', so the emulator was serving the corpus. None of the six clusters below carries a stub_path or an owned_by.

Cause 1 - [mismatch] textract-adapter-and-resource-error-paths#task#6, TagResource: expected HTTP 400 and the emulator returned HTTP 400, so only the body diverges - $.Message: value_mismatch (expected 'Adapter Id not found', got 'Adapter Id not found.'). $.__type is not among the diverging keys, so the error code matched. divergence_key service=textract|TagResource|status=400->400|keys=$.Message.

Cause 2 - [mismatch] textract-adapter-and-resource-error-paths#task#7, ListTagsForResource: status 400 as expected; $.Message: value_mismatch (expected 'Adapter Id not found', got 'Adapter Id not found.'). divergence_key service=textract|ListTagsForResource|status=400->400|keys=$.Message.

Cause 3 - [mismatch] textract-adapter-and-resource-error-paths#task#8, UntagResource: status 400 as expected; $.Message: value_mismatch (expected 'Adapter Id not found', got 'Adapter Id not found.'). divergence_key service=textract|UntagResource|status=400->400|keys=$.Message. In all three, the ONLY difference is the trailing '.' the emulator appends; the preceding characters are byte-identical.

Cause 4 - [mismatch] textract-async-start-and-get-error-paths#task#2, GetDocumentTextDetection: status 400 as expected; $.Message: value_mismatch (expected 'Request has invalid Job Id', got 'a4d2ec400ad44722b55c156962555d3e5d07b36941b840f8814d07f707ad991d is not a text detection job started by this service'). The 64-hex leading token is the JobId that step sent and is a per-run value, NOT part of the requirement; the requirement is the constant expected string, and the emulator's fixed part is ' is not a text detection job started by this service'. divergence_key service=textract|GetDocumentTextDetection|status=400->400|keys=$.Message.

Cause 5 - [mismatch] textract-async-start-and-get-error-paths#task#4, GetDocumentAnalysis: status 400 as expected; $.Message: value_mismatch (expected 'Request has invalid Job Id', got 'a4d2ec400ad44722b55c156962555d3e5d07b36941b840f8814d07f707ad991d is not a document analysis job started by this service'). Same structure as Cause 4 with 'document analysis' substituted for 'text detection'. divergence_key service=textract|GetDocumentAnalysis|status=400->400|keys=$.Message.

Cause 6 - [mismatch] textract-sync-analyze-error-paths#task#2, AnalyzeDocument: status 400 as expected; $.Message: value_mismatch (expected 'Request has invalid parameters', got 'FeatureTypes must contain at least one FeatureType'). Both strings are wholly fixed text - no request value appears in either. divergence_key service=textract|AnalyzeDocument|status=400->400|keys=$.Message.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator from it, and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus with no arguments).
2. Cause 1: replay fixture textract-adapter-and-resource-error-paths, step task#6 - TagResource with a well-formed adapter ResourceARN that names no existing adapter. Observe HTTP 400 with the right error code but Message 'Adapter Id not found.' instead of 'Adapter Id not found'.
3. Causes 2 and 3: replay the same fixture's steps task#7 (ListTagsForResource) and task#8 (UntagResource) against the same unknown adapter ARN; both show the same appended period.
4. Cause 4: replay fixture textract-async-start-and-get-error-paths, step task#2 - GetDocumentTextDetection with a syntactically valid JobId (matching ^[a-zA-Z0-9-_]{1,64}$) that the emulator never issued. Observe InvalidJobIdException at HTTP 400 whose Message interpolates the submitted JobId instead of the fixed 'Request has invalid Job Id'.
5. Cause 5: replay the same fixture's step task#4 - GetDocumentAnalysis with the same never-issued JobId; same defect with 'document analysis' wording.
6. Cause 6: replay fixture textract-sync-analyze-error-paths, step task#2 - AnalyzeDocument with a valid Document and an EMPTY FeatureTypes list. Observe InvalidParameterException at HTTP 400 with Message 'FeatureTypes must contain at least one FeatureType' instead of 'Request has invalid parameters'.
7. After fixing, re-replay the representative step (textract-adapter-and-resource-error-paths#task#6) AND every fixture#step token in trace_ids; each must return the recorded status, error code and the exact expected Message, and the 19 steps that already pass must still pass.
