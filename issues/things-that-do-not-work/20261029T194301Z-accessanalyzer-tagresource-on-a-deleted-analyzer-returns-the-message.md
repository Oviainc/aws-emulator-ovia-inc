Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.delete-analyzer-removes-resource
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.delete-analyzer-removes-resource|run#15|AssertionError|assert tag_message == "Resource was not found", tag_message
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# TagResource on a deleted analyzer returns the message "Resource not found" instead of AWS's "Resource was not found"

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
REOPENS 20261023T110741Z-accessanalyzer-access-analyzer-rejections-use-the-wrong-error-code.md: its fix left the call refused. That issue's Cause 1 (TagResource on a dead analyzer answering ResourceNotFoundException instead of NotFoundException) and its message-text causes were marked fixed; the error CODE is now correct (NotFoundException), but the refusal still carries emulator-authored text rather than AWS's constant, which is the same defect class that issue was filed to close.

One cause.

Cause 1 of 1: the NotFoundException raised by TagResource against a deleted analyzer ARN carries the message "Resource not found" where AWS returns "Resource was not found".
AWS: Smithy (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json): TagResource is POST /tags/{resourceArn} with resourceArn @httpLabel @required and tags @required, and declares ResourceNotFoundException (HTTP 404) among its errors — the model says nothing about NotFoundException or about message text, so for this refusal it is recorded behavior only: against a deleted analyzer's ARN real AWS answers the error code NotFoundException with the fixed, request-independent message "Resource was not found". The whole string is fixed text; it interpolates no resource name, ARN, account or id, so it does not vary with the request or with environment.
Emulator: the status and the error code now match, but the message is "Resource not found" — the same meaning in different words, produced by the tag-path not-found helper (crate::state::tag_target_not_found(), returned at services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/tag_resource.rs line 179).
Why: the message is part of the contract for this error class. A client, test fixture or log scraper that matches AWS's constant "Resource was not found" — common where the error code alone is too coarse to tell a dead tag target apart from other 404s — does not match, so the branch written for this refusal is never taken; and anything that reproduces AWS messages (recorded-fixture comparisons, user-facing error surfacing) diverges on text the service is supposed to hold constant.
Fix: the message on the tag-path not-found refusal — crate::state::tag_target_not_found() in services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/state.rs, as used by TagResource's not-found path (and UntagResource, if it shares the helper). The error code must stay NotFoundException; only the fixed text changes.
Traces: accessanalyzer.delete-analyzer-removes-resource.

## Evidence
Check accessanalyzer.delete-analyzer-removes-resource [mismatch]. The check reached the emulator and was answered; everything before this assertion passes — DeleteAnalyzer succeeds, GetAnalyzer then refuses with code ResourceNotFoundException and message "Analyzer not found", ListAnalyzers no longer lists the analyzer, and TagResource's error code assertion (NotFoundException) now passes.

Failing assertion (check file line 41):
  assert tag_message == "Resource was not found", tag_message
  AssertionError: Resource not found

Call under test: TagResource(resourceArn=<the deleted analyzer's arn>, tags={'probe': 'post-delete'}).
Expected (recorded AWS): error code NotFoundException with message "Resource was not found" (entirely fixed text — no interpolated values).
Actual (emulator): error code NotFoundException with message "Resource not found".

Source evidence: services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/tag_resource.rs line 179 returns crate::state::tag_target_not_found() when analyzer_by_arn_mut finds no analyzer; that helper in src/state.rs supplies the diverging text.

## Steps to Reproduce
1. Boot the emulator and point an accessanalyzer client at it.
2. CreateAnalyzer(analyzerName=N, type='ACCOUNT'); poll GetAnalyzer until status is ACTIVE; note the returned arn A.
3. DeleteAnalyzer(analyzerName=N); poll GetAnalyzer until it refuses (ResourceNotFoundException / "Analyzer not found").
4. TagResource(resourceArn=A, tags={'probe': 'post-delete'}).
5. Read the error: the emulator answers code NotFoundException with message "Resource not found"; real AWS answers code NotFoundException with message "Resource was not found".
