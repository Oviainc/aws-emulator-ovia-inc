Issue-Source: trace-sweep-divergence
Issue-Trace: translate-translate-document
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-translate-document
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|TranslateDocument|4xx->4xx|status
Issue-Commit: 3fda3c666cd20fe501445ba9b697a49b3557997c

# TranslateDocument not-found error answers HTTP 404 where AWS answers 400

> _Note: this issue was identified against the code base at commit `3fda3c666cd20fe501445ba9b697a49b3557997c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TranslateDocument

## Description
One net-new cause in the Amazon Translate error path, on the TranslateDocument operation. It is the same family of fault already reported for GetTerminology/DeleteTerminology/TranslateText in the open issue 20261029T184304Z-translate-error-responses-use-wrong-status-and-message.md (and that issue does NOT carry this fixture step, so this divergence is not covered by it): the emulator picks the 404 ResourceNotFoundException class for a Translate request error that AWS answers as a 400 client-validation error. Here, unlike those causes, the response BODY already matches the recording byte-for-byte — the only diverging key is the HTTP status — so the defect is purely the error-class/status mapping of this operation's failure path.

Cause 1 of 1 - TranslateDocument rejecting a request whose referenced resource/arguments are not valid in the account's state.
AWS: recorded behavior only. The Smithy model binds both a 400 and a 404 error to this operation, so the model alone does not pick one (Smithy: TranslateDocument errors = InternalServerException 500, InvalidRequestException 400, LimitExceededException 400, ResourceNotFoundException 404, ServiceUnavailableException 503, TooManyRequestsException 429, UnsupportedLanguagePairException 400). The recording shows AWS answering HTTP 400 for this request, with the error message the emulator already reproduces: its fixed text is the error-class sentence frame AWS emits for this failure, and its variable parts are the caller's 12-digit account id and the resource name / language codes taken from the request and the emulator's state. Those variable values (account id, generated terminology name) are environment values that fill the template, not requirements.
Emulator: answers HTTP 404 (ResourceNotFoundException) for the identical request. Replay reports `status: expected 400, got 404` and reports NO body-key divergence, so the message and the rest of the payload are already correct.
Why: a client that branches on the HTTP status, or on the modeled error shape the status selects, sees a 404/ResourceNotFoundException where AWS raises a 400 request error. SDK retry classification differs between the two (a 404 is a resource-absent condition, a 400 is a non-retryable client error), and conformance tests or error-code assertions keyed on InvalidRequestException take the wrong branch even though the human-readable message is identical.
Fix: the TranslateDocument handler's failure path in the translate service crate (services/service-com-amazonaws-translate-awsshinefrontendservice-20170701) — it must raise the 400 client-error class AWS uses for this condition instead of the 404 ResourceNotFoundException class. The message formatting is already right and must not change; only the error class and the HTTP status it serializes to need to.
Traces: translate-translate-document#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct=34.1).

Cause 1 - fixture translate-translate-document, step task#3, operation TranslateDocument: expected status 400, actual status 404. Divergence key `service=translate|TranslateDocument|status=400->404|keys=status`; verdict mismatch; affected_count 1 (no other cluster members). The divergence lists `status` as the ONLY diverging key: every compared body member matched the recording, so the emulator's error message text is already AWS-faithful and the 404 vs 400 status (and therefore the error class it encodes) is the whole divergence.

This step carried no stub_path and no owned_by in the replay report, i.e. it is not blocked by an unimplemented operation and no open issue carries its replay key.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Fixture translate-translate-document, step task#3: issue the recorded TranslateDocument request.
4. Observe the response status is 404 (ResourceNotFoundException) while the recording shows AWS answering 400; compare the body, which already matches, to confirm only the status/error class diverges.
