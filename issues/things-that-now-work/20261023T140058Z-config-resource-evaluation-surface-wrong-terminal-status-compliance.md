Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.resource-evaluation-summary-reflects-in-progress-then-complete config.resource-evaluation-summary-ttl-expiry config.resource-evaluation-summary-reflects-proactive-evaluation config.e2e-resource-evaluation-summary-flow config.start-resource-evaluation-idempotency-mismatch-rejected config.start-resource-evaluation-idempotent-parameter-mismatch-rejected config.proactive-resource-evaluation-reaches-terminal-state config.get-resource-evaluation-summary-unknown-token-rejected config.list-resource-evaluations-time-range-ordering
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.resource-evaluation-summary-reflects-in-progress-then-complete|run#18|AssertionError|assert terminal["Compliance"] == "NOT_APPLICABLE", (
Issue-Key: config.resource-evaluation-summary-ttl-expiry|run#12|AssertionError|assert final_summary["Compliance"] == "NOT_APPLICABLE", (
Issue-Key: config.resource-evaluation-summary-reflects-proactive-evaluation|run#14|AssertionError|assert status == "FAILED", (
Issue-Key: config.e2e-resource-evaluation-summary-flow|run#18|AssertionError|assert set(match) == {"ResourceEvaluationId", "EvaluationStartTimestamp"}, (
Issue-Key: config.start-resource-evaluation-idempotency-mismatch-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: config.start-resource-evaluation-idempotent-parameter-mismatch-rejected|run#13|AssertionError|assert message == expected_message, (
Issue-Key: config.proactive-resource-evaluation-reaches-terminal-state|run#16|AssertionError|assert mismatch_msg == (
Issue-Key: config.get-resource-evaluation-summary-unknown-token-rejected|run#12|AssertionError|assert message == f"Provided ResourceEvaluationId {fake_id} not found.", message
Issue-Key: config.list-resource-evaluations-time-range-ordering|run#22|AssertionError|assert message == "", f"expected an empty InvalidTimeRangeException message, got {message!r}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config resource-evaluation surface: wrong terminal Status/Compliance for proactive evaluations, extra EvaluationMode in ListResourceEvaluations, and three diverging refusal messages (5 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartResourceEvaluation

## Description
FIVE CAUSES must be closed on the StartResourceEvaluation / GetResourceEvaluationSummary / ListResourceEvaluations surface, all of which answers.

Cause 1 of 5: the terminal Status/Compliance of a proactive resource evaluation with no applicable PROACTIVE rule.
AWS: GetResourceEvaluationSummary returns EvaluationStatus (ResourceEvaluationStatus: IN_PROGRESS | FAILED | SUCCEEDED) and Compliance (ComplianceType: COMPLIANT | NON_COMPLIANT | NOT_APPLICABLE | INSUFFICIENT_DATA) (Smithy: GetResourceEvaluationSummary). Recorded behavior: in an account with no PROACTIVE rule for the evaluated resource type, the evaluation does not end as a successful evaluation with unknown data — it reports Compliance NOT_APPLICABLE where it succeeds, and in the bare-account leg it terminates with Status FAILED.
Emulator: reports Compliance INSUFFICIENT_DATA for the SUCCEEDED legs, and Status SUCCEEDED where AWS terminates FAILED.
Why: the three terminal verdicts mean different things to a client: NOT_APPLICABLE means "no rule applies", INSUFFICIENT_DATA means "a rule applies but could not decide", FAILED means the evaluation did not run. A CI gate reading the emulator sees an inconclusive-but-successful evaluation where AWS says either "nothing to evaluate" or "the evaluation failed".
Fix: the terminal-state computation of the proactive resource-evaluation state machine (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_resource_evaluation.rs and the evaluation-summary state behind .../src/ops/get_resource_evaluation_summary.rs).
Traces: config.resource-evaluation-summary-reflects-in-progress-then-complete, config.resource-evaluation-summary-ttl-expiry, config.resource-evaluation-summary-reflects-proactive-evaluation.

Cause 2 of 5: ListResourceEvaluations entries carry EvaluationMode.
AWS: ResourceEvaluation models ResourceEvaluationId, EvaluationMode and EvaluationStartTimestamp (Smithy: ResourceEvaluation), but recorded behavior shows AWS populating only ResourceEvaluationId and EvaluationStartTimestamp in ListResourceEvaluations entries — the mode lives in the request's Filters vocabulary, not in the listed item.
Emulator: each entry also carries EvaluationMode.
Why: a client that enumerates the entry's members (round-trip comparisons, snapshot assertions, strict deserializers that reject unexpected populated members in fixtures) sees a shape AWS does not produce.
Fix: the ListResourceEvaluations serializer — do not populate EvaluationMode on listed entries (.../src/ops/list_resource_evaluations.rs).
Traces: config.e2e-resource-evaluation-summary-flow.

Cause 3 of 5: the idempotency-mismatch refusal message of StartResourceEvaluation.
AWS: StartResourceEvaluation models IdempotentParameterMismatch (client/400) for a ClientToken (@length 64..256) reused with different parameters (Smithy: StartResourceEvaluation). Recorded behavior, confirmed by three independent recordings with three different tokens, is one template: "The request inputs are not associated with the supplied client token <token>. You are using the same client token with one or more different parameters. Specify a new client token with the parameter changes and try again." — the only request-derived part is the token.
Emulator: same code, a different template: "The ClientToken '<token>' provided in the request already started a resource evaluation with different parameters. Specify a new client token with the parameter changes and try again."
Why: the message template is the contract here (the token value is just its parameter); clients and operators that match AWS's sentence do not match the emulator's.
Fix: the IdempotentParameterMismatch message built by the StartResourceEvaluation handler's client-token bookkeeping (.../src/ops/start_resource_evaluation.rs).
Traces: config.start-resource-evaluation-idempotency-mismatch-rejected, config.start-resource-evaluation-idempotent-parameter-mismatch-rejected, config.proactive-resource-evaluation-reaches-terminal-state.

Cause 4 of 5: the GetResourceEvaluationSummary not-found message.
AWS: the operation models exactly ResourceNotFoundException (client/400) for an unknown ResourceEvaluationId (Smithy: GetResourceEvaluationSummary). Recorded template: "Provided ResourceEvaluationId <id> not found."
Emulator: same code, text "The ResourceEvaluationId '<id>' provided in the request is invalid. Please check the resource evaluation ID."
Why: the refusal reads as an input-validity complaint rather than a not-found, and the fixed template clients match on is absent.
Fix: the ResourceNotFoundException message in the GetResourceEvaluationSummary handler (.../src/ops/get_resource_evaluation_summary.rs).
Traces: config.get-resource-evaluation-summary-unknown-token-rejected.

Cause 5 of 5: ListResourceEvaluations InvalidTimeRangeException carries a message where AWS carries none.
AWS: ListResourceEvaluations models InvalidTimeRangeException (client/400) for Filters.TimeWindow with StartTime after EndTime (Smithy: ListResourceEvaluations). Recorded behavior: the error is returned with an EMPTY message — the code alone carries the information.
Emulator: fills in "The specified time range is not valid. Filters.TimeWindow.StartTime must not be after Filters.TimeWindow.EndTime."
Why: clients that assert on the recorded envelope (empty message) see an extra field value; the divergence is in whether the member is populated at all, not merely in wording.
Fix: the InvalidTimeRangeException construction in the ListResourceEvaluations handler (.../src/ops/list_resource_evaluations.rs).
Traces: config.list-resource-evaluations-time-range-ordering.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

Cause 1:
  config.resource-evaluation-summary-reflects-in-progress-then-complete: AssertionError: a SUCCEEDED evaluation with no PROACTIVE rule for the resource type must report Compliance NOT_APPLICABLE, got 'INSUFFICIENT_DATA'
  config.resource-evaluation-summary-ttl-expiry: same assertion, same emulator value.
  config.resource-evaluation-summary-reflects-proactive-evaluation: AssertionError: a bare account with no PROACTIVE rule for AWS::S3::Bucket must terminate the evaluation FAILED, got 'SUCCEEDED' ({'Status': 'SUCCEEDED'})

Cause 2:
  config.e2e-resource-evaluation-summary-flow: AssertionError: expected a ListResourceEvaluations entry with exactly ResourceEvaluationId and EvaluationStartTimestamp (no EvaluationMode), got ['EvaluationMode', 'EvaluationStartTimestamp', 'ResourceEvaluationId']

Cause 3 (three recordings, three different tokens, one emulator template):
  config.start-resource-evaluation-idempotency-mismatch-rejected: expected "The request inputs are not associated with the supplied client token <token>. You are using the same client token with one or more different parameters. ...", got "The ClientToken '<token>' provided in the request already started a resource evaluation with different parameters. ..."
  config.start-resource-evaluation-idempotent-parameter-mismatch-rejected: same divergence.
  config.proactive-resource-evaluation-reaches-terminal-state: same emulator text.

Cause 4:
  config.get-resource-evaluation-summary-unknown-token-rejected: expected "Provided ResourceEvaluationId <id> not found.", got "The ResourceEvaluationId '<id>' provided in the request is invalid. Please check the resource evaluation ID."

Cause 5:
  config.list-resource-evaluations-time-range-ordering: AssertionError: expected an empty InvalidTimeRangeException message, got 'The specified time range is not valid. Filters.TimeWindow.StartTime must not be after Filters.TimeWindow.EndTime.'

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — ResourceEvaluationStatus = IN_PROGRESS|FAILED|SUCCEEDED; ComplianceType = COMPLIANT|NON_COMPLIANT|NOT_APPLICABLE|INSUFFICIENT_DATA; StartResourceEvaluation errors IdempotentParameterMismatch (client, httpError 400) and InvalidParameterValueException; GetResourceEvaluationSummary errors ResourceNotFoundException; ListResourceEvaluations errors InvalidNextTokenException/InvalidParameterValueException/InvalidTimeRangeException.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
2. Cause 1: config start-resource-evaluation --evaluation-mode PROACTIVE for an AWS::S3::Bucket in an account with no PROACTIVE rule, then poll config get-resource-evaluation-summary — observe EvaluationStatus/Compliance.
3. Cause 2: config list-resource-evaluations and inspect the members of each ResourceEvaluations entry.
4. Cause 3: call config start-resource-evaluation twice with the same --client-token and different ResourceDetails; read the IdempotentParameterMismatch message.
5. Cause 4: config get-resource-evaluation-summary --resource-evaluation-id <random uuid>; read the ResourceNotFoundException message.
6. Cause 5: config list-resource-evaluations with Filters.TimeWindow.StartTime after EndTime; inspect the error envelope's message member.
