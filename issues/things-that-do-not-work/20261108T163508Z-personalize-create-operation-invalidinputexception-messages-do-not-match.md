Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-campaign-and-batch-jobs-nonexistent-solution
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-campaign-and-batch-jobs-nonexistent-solution personalize-filter-and-recommender-validation
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize|CreateCampaign|4xx->4xx|$.message
Issue-Key: personalize|CreateFilter|4xx->4xx|$.message
Issue-Commit: e0a3636fd96de616fa27c30a9c0561cdbb99234b

# Personalize create-operation InvalidInputException messages do not match AWS wording (2 causes)

> _Note: this issue was identified against the code base at commit `e0a3636fd96de616fa27c30a9c0561cdbb99234b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCampaign

## Description
Two Personalize control-plane create operations reject a request with the correct HTTP status (400) and the correct error code (InvalidInputException), but with a message that is not the message AWS produces for that error class. In both cases the emulator substitutes a generic or emulator-internal sentence for AWS's specific, documented reason, so the response no longer tells a caller WHY the request failed. This is one shared fault — the per-operation validation paths build their own ad-hoc message text instead of AWS's wording for the condition they detected.

Cause 1 of 2: CreateCampaign against a solution-version ARN that does not exist.
AWS: Smithy (com-amazonaws-personalize-amazonpersonalize): CreateCampaign.solutionVersionArn is @required, target Arn, @length max 256, @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$; modeled errors include InvalidInputException (httpError 400) and ResourceNotFoundException (httpError 404). The model does not state the message text, so for the text this is recorded behavior only: the recording shows AWS answering HTTP 400 InvalidInputException whose message is the fixed text "The given solution does not exist: " followed by one variable part — the solutionVersionArn value taken verbatim from the request. Note AWS classifies this as InvalidInputException (400), not ResourceNotFoundException (404).
Emulator: answers HTTP 400 with the same error code but the message "Resource Arn <arn> does not exist." — i.e. a generic resource-shaped sentence (fixed text "Resource Arn " + the ARN + " does not exist.") that names neither the solution nor the fact that it is the campaign's solution version that could not be resolved. Replay detail: $.message value_mismatch (expected 'The given solution does not exist: <solutionVersionArn>', got 'Resource Arn <solutionVersionArn> does not exist.').
Why: a client that distinguishes a missing solution from other invalid input — retry/provision-then-retry logic, or a test asserting on the documented message — sees a message that matches no AWS error string, so it cannot classify the failure and falls through to a generic error path even though the request failed for a known, recoverable reason.
Fix: CreateCampaign's input validation / error construction in the personalize service crate — the branch that resolves solutionVersionArn must raise InvalidInputException with AWS's "The given solution does not exist: {solutionVersionArn}" template rather than reusing the shared generic "Resource Arn {arn} does not exist." formatter.
Traces: personalize-campaign-and-batch-jobs-nonexistent-solution#task#1.

Cause 2 of 2: CreateFilter with a filterExpression that references the INTERACTIONS dataset in a dataset group that has no completed INTERACTIONS import.
AWS: Smithy: CreateFilter.filterExpression is @required, @sensitive, @length 1..2500; datasetGroupArn is @required; modeled errors include InvalidInputException (httpError 400). The model does not state the message text, so for the text this is recorded behavior only: the recording shows AWS answering HTTP 400 InvalidInputException with the entirely fixed text "Dataset group must contain an INTERACTIONS dataset with successfully completed dataset import job or EVENT_INTERACTIONS" — there are no request- or state-derived variable parts in this message. The requirement AWS is expressing is a state precondition on the dataset group (an INTERACTIONS dataset with a successfully completed import job, or EVENT_INTERACTIONS), not a restriction on the expression language: an interactions-referencing filter expression is legal once that precondition holds.
Emulator: answers HTTP 400 with the same error code but the message "Invalid filter expression: filter expression references the INTERACTIONS dataset; this emulator evaluates filter expressions over Items and Actions only" — emulator-internal text that reports an unimplemented capability instead of the dataset-group precondition, and that would keep rejecting the request even after the dataset group did contain a completed INTERACTIONS import. Replay detail: $.message value_mismatch.
Why: the response tells a client the filter expression itself is unsupported rather than that its dataset group is not yet populated, so the documented remedy (import INTERACTIONS data, or use EVENT_INTERACTIONS, then re-create the filter) looks futile; interaction-based filters are unreachable in the emulator by construction and the error text advertises the emulator rather than AWS. Related open issue 20261024T052643Z-personalize-runtime-getrecommendations-event-propagation.md notes interaction-based filters being rejected outright, but it does not carry this fixture step's replay key, so this divergence is filed here.
Fix: CreateFilter's validation plus the filter-expression evaluator in the personalize service crate — gate interactions-referencing expressions on the dataset group's INTERACTIONS/EVENT_INTERACTIONS state and emit AWS's fixed message for that precondition, instead of rejecting the expression in the parser with emulator-specific text.
Traces: personalize-filter-and-recommender-validation#task#3, personalize-filter-and-recommender-validation#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200): 53 steps, 36 passed, score 67.9%.

Cause 1 — fixture personalize-campaign-and-batch-jobs-nonexistent-solution, step task#1, operation CreateCampaign, verdict mismatch.
  expected_status 400, actual_status 400 (error code matches; only $.message diverges)
  expected $.message: 'The given solution does not exist: arn:aws:personalize:us-west-2:000000000000:solution/<name>/<name>'
  actual   $.message: 'Resource Arn arn:aws:personalize:us-west-2:000000000000:solution/<name>/<name> does not exist.'
  (the ARN shown is the per-run value from the request; the requirement is the fixed text around it)
  affected_count 1.

Cause 2 — fixture personalize-filter-and-recommender-validation, step task#3, operation CreateFilter, verdict mismatch.
  expected_status 400, actual_status 400 (error code matches; only $.message diverges)
  expected $.message: 'Dataset group must contain an INTERACTIONS dataset with successfully completed dataset import job or EVENT_INTERACTIONS'
  actual   $.message: 'Invalid filter expression: filter expression references the INTERACTIONS dataset; this emulator evaluates filter expressions over Items and Actions only'
  affected_count 2: personalize-filter-and-recommender-validation#task#3, personalize-filter-and-recommender-validation#task#4.

Model evidence: smithy_query operation=CreateCampaign,CreateFilter on services/service-com-amazonaws-personalize-amazonpersonalize/smithy-model/com-amazonaws-personalize-amazonpersonalize.json — both operations model InvalidInputException with httpError 400; CreateCampaign.solutionVersionArn @required @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$; CreateFilter.filterExpression @required @sensitive @length 1..2500. Message text is not modeled, so the expected strings above are the recorded AWS behavior.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus() against the built emulator (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture personalize-campaign-and-batch-jobs-nonexistent-solution, step task#1 — CreateCampaign with a syntactically valid solutionVersionArn for a solution that was never created. Observe HTTP 400 InvalidInputException whose message is 'Resource Arn <arn> does not exist.' instead of 'The given solution does not exist: <solutionVersionArn>'.
3. Cause 2: replay fixture personalize-filter-and-recommender-validation, steps task#3 and task#4 — CreateFilter against a dataset group with no completed INTERACTIONS import, using a filterExpression that references the INTERACTIONS dataset. Observe HTTP 400 InvalidInputException whose message is the emulator-internal 'Invalid filter expression: ... this emulator evaluates filter expressions over Items and Actions only' instead of AWS's 'Dataset group must contain an INTERACTIONS dataset with successfully completed dataset import job or EVENT_INTERACTIONS'.
4. Re-replay both fixture steps after the fix; both must match on $.message with status 400 and error code InvalidInputException.
