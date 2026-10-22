Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-analyzer-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-analyzer-lifecycle accessanalyzer-policy-check-apis
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|UpdateAnalyzer|4xx->4xx|$.message
Issue-Key: access-analyzer|UpdateAnalyzer|4xx->4xx|$.reason
Issue-Key: access-analyzer|CheckNoNewAccess|2xx->2xx|$.message
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# access-analyzer UpdateAnalyzer and CheckNoNewAccess emit non-AWS response field values

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAnalyzer

## Description
Replay of the access-analyzer wire fixtures surfaced 2 net-new causes that share one fault family: the handler reaches the correct HTTP status and the correct modeled member set, but populates a machine-readable response member with the emulator's own vocabulary instead of the value AWS puts on the wire. Both causes must be closed before this issue is submitted.

Cause 1 of 2: UpdateAnalyzer's ValidationException carries the wrong ValidationExceptionReason value.
AWS: Smithy (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json): UpdateAnalyzer PUT /analyzer/{analyzerName}, errors include ValidationException (errorKind client, httpError 400); ValidationException.message is @required and ValidationException.reason: ValidationExceptionReason is @required; the enum admits unknownOperation, cannotParse, fieldValidationFailed, other, notSupported. The recorded AWS response for rejecting this UpdateAnalyzer request is HTTP 400 with reason='FIELD_VALIDATION_FAILED' (recorded behavior only: AWS puts the SCREAMING_SNAKE form of the discriminator on the wire, which is not the literal spelling listed in the model's enum, so the recording is the authority for the wire value here).
Emulator: answers the right status (400) and does serialize reason, but with the value 'notSupported' instead of 'FIELD_VALIDATION_FAILED'. (Its message also differs - recorded example 'Unsupported operation' vs the emulator's 'UpdateAnalyzer is not supported for an analyzer of type ACCOUNT'. No rule for that wording is documented or request-derived, so the recorded text is treated as an example and the wording alone is NOT the defect filed here.)
Why: reason is the machine-readable discriminator clients branch on to classify a ValidationException. A client that retries or re-forms the request on fieldValidationFailed, and surfaces a hard 'this operation is not supported' error on notSupported, takes the opposite branch against the emulator than it takes against AWS - and the divergence is invisible in the status code, so it only shows up as wrong client control flow.
Fix: the ValidationException construction on the access-analyzer UpdateAnalyzer handler/validation path - the rejection must be raised with the field-validation reason AWS uses, serialized in the wire spelling the recording shows, rather than the emulator's own notSupported classification.
Traces: accessanalyzer-analyzer-lifecycle#task#8.

Cause 2 of 2: CheckNoNewAccess returns a different fixed result message for a FAIL result.
AWS: Smithy: CheckNoNewAccess POST /policy/check-no-new-access, output CheckNoNewAccessResponse with members result: CheckNoNewAccessResult, message: String ('The message indicating whether the updated policy allows new access.') and reasons: ReasonSummaryList. HTTP 200. Recorded behavior only: for a FAIL result AWS returns the fixed sentence 'The modified permissions grant new access compared to your existing policy.' - a service-side constant tied to the result value, containing no host, id, timestamp or other environment-dependent value, so it is reproducible rather than an incidental recording artifact.
Emulator: answers 200 with the matching result and reasons, but $.message is the shorter 'The modified permissions grant new access.'
Why: message is the human-readable verdict a client renders or asserts on for a policy-check gate; a client or CI check that matches the AWS sentence passes against AWS and fails only here, and the two texts are not substitutable.
Fix: the CheckNoNewAccess handler's result-to-message mapping (the message emitted for the FAIL result), in the access-analyzer policy-check path.
Traces: accessanalyzer-policy-check-apis#task#4.

Related but NOT covering these: the open issue '20261022T030924Z-access-analyzer-read-and-error-paths-diverge-from.md' covers 7 other access-analyzer causes (date-time timestamp encoding on GetAnalyzer/ListAnalyzers/GetArchiveRule/ListArchiveRules, the deleted archive rule still served with 200, the MISSING reason member on ListPolicyGenerations, and the dropped empty tags map). It carries no replay key for accessanalyzer-analyzer-lifecycle#task#8 or accessanalyzer-policy-check-apis#task#4, and Cause 1 here is a reason member that IS present with the wrong value (not absent), on a different operation. Replay reported state_reset_failures=0, so neither cause is contaminated by a previous fixture's state.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 66 steps: 21 passed / 45 mismatched / 0 errored / 0 blocked, score 31.8%, 9 divergence clusters). Both clusters below are service=access-analyzer with stub_path empty and owned_by empty (the other 7 clusters are owned by 20261022T030924Z-access-analyzer-read-and-error-paths-diverge-from.md and are not re-filed).

Cause 1 - UpdateAnalyzer, fixture accessanalyzer-analyzer-lifecycle, step task#8, status 400 -> 400, divergence_key service=access-analyzer|UpdateAnalyzer|status=400->400|keys=$.message,$.reason:
  $.reason: value_mismatch (expected 'FIELD_VALIDATION_FAILED', got 'notSupported')   <- the defect: machine-readable discriminator, wrong value
  $.message: value_mismatch (expected 'Unsupported operation', got 'UpdateAnalyzer is not supported for an analyzer of type ACCOUNT')   <- recorded wording treated as an example, not a requirement
  Status and the presence of both required members agree; only the reason value diverges.

Cause 2 - CheckNoNewAccess, fixture accessanalyzer-policy-check-apis, step task#4, status 200 -> 200, divergence_key service=access-analyzer|CheckNoNewAccess|status=200->200|keys=$.message:
  $.message: value_mismatch (expected 'The modified permissions grant new access compared to your existing policy.', got 'The modified permissions grant new access.')
  No other member of CheckNoNewAccessResponse diverged (result and reasons matched).

Model citations (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json): UpdateAnalyzer -> PUT /analyzer/{analyzerName}, errors [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; ValidationException members message @required, reason: ValidationExceptionReason @required, fieldList; ValidationExceptionReason values [unknownOperation, cannotParse, fieldValidationFailed, other, notSupported]; CheckNoNewAccess -> POST /policy/check-no-new-access, output CheckNoNewAccessResponse{result: CheckNoNewAccessResult, message: String, reasons: ReasonSummaryList}, HTTP 200.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (replay_wire_corpus), then re-run these two steps per cause:

1. Cause 1: fixture accessanalyzer-analyzer-lifecycle, step task#8 - create an ACCOUNT-type analyzer, then PUT /analyzer/{analyzerName} (UpdateAnalyzer) with the fixture's configuration. Observe HTTP 400 whose body has $.reason = 'notSupported'; expected $.reason = 'FIELD_VALIDATION_FAILED' as recorded from AWS. The accompanying message text is an example only - do not assert on it.
2. Cause 2: fixture accessanalyzer-policy-check-apis, step task#4 - POST /policy/check-no-new-access with the fixture's newPolicyDocument/existingPolicyDocument/policyType, producing a FAIL result. Observe $.message = 'The modified permissions grant new access.'; expected the AWS fixed sentence 'The modified permissions grant new access compared to your existing policy.' with the same result and reasons.

Both must be re-replayed green before closing. Replay reported state_reset_failures=0, so neither step ran against a previous fixture's state.
