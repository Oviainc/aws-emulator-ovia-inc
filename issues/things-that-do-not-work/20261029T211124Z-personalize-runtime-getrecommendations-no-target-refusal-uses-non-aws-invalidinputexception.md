Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-runtime-recommendations-input-validation-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-runtime-recommendations-input-validation-errors
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime|GetRecommendations|4xx->4xx|$.message
Issue-Commit: 0622c476c3133f3ae7e7fc8f4e228880242e86b0

# GetRecommendations no-target refusal uses non-AWS InvalidInputException message text

> _Note: this issue was identified against the code base at commit `0622c476c3133f3ae7e7fc8f4e228880242e86b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
Cause 1 of 1: the InvalidInputException raised when a GetRecommendations request names neither an inference target carries the emulator's own wording instead of AWS's.

AWS: neither `campaignArn` nor `recommenderArn` is @required in the model (either resource type may back the operation), so a request that supplies neither is refused at validation with InvalidInputException, HTTP 400, and the fixed message text `campaignArn or recommenderArn required.` Smithy: GetRecommendations input members campaignArn / recommenderArn both target `Arn` (@length 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), neither @required; modeled errors InvalidInputException (400) and ResourceNotFoundException (404). The exact text is recorded behavior only; the rule the recording establishes is that this condition has its OWN fixed message, distinct from the ARN-resolution message (`<arn> does not exist or is not active yet.`) returned once a target IS named, because the error code and status are identical for both and the message is the only observable that separates them. This message has no variable parts: it is fixed text end to end, with no request- or state-derived substitution (contrast the resolution message, whose leading `<arn>` is the caller's ARN).

Emulator: status 400 and code InvalidInputException match, but the message body is `Either campaignArn or recommenderArn is required.` — a reworded equivalent (leading `Either `, `is required` instead of `required`). The replayed step reports exactly one diverging key, $.message.

Why: InvalidInputException is the single code personalize-runtime uses for "you named no target", "the target you named does not exist" and "that ARN is not a campaign ARN". A client, SDK wrapper or error-classifying middleware that discriminates those branches can only do so on the message string; against the emulator the no-target branch matches none of its patterns, so a plain missing-parameter programming error is misclassified as a resource/availability problem and may be retried instead of surfaced.

Fix: the validation arm of the request resolution stage for GetRecommendations in the personalize-runtime crate — the branch that handles "neither campaignArn nor recommenderArn supplied" — must emit AWS's fixed text verbatim. The sibling operations that share the same resolution helper should emit the same literal.

Traces: fixture personalize-runtime-recommendations-input-validation-errors, steps task#2 and task#3 (one cluster, two occurrences, same diverging key $.message).

Related but NOT covering this: the open behavioral issue `20261023T071630Z-personalize-runtime-cannot-distinguish-its-rejection-reasons-a.md` describes the same wording gap as its Cause 2, but it is filed against the behavioral corpus and carries no wire replay key, so this wire fixture step is un-gated by it.

## Evidence
Replayed step personalize-runtime-recommendations-input-validation-errors#task#2 (operation GetRecommendations, verdict mismatch): expected HTTP 400, actual HTTP 400 — status and error code (InvalidInputException) agree. Sole diverging key: $.message — value_mismatch, expected `campaignArn or recommenderArn required.`, got `Either campaignArn or recommenderArn is required.`. The whole expected string is fixed text (no request- or state-derived values in it). Same divergence recurs at step task#3 of the same fixture (affected_count 2, divergence_key service=personalize-runtime|GetRecommendations|status=400->400|keys=$.message). No stub_path and no owning issue were reported for this cluster.

## Steps to Reproduce
1. Boot the emulator and confirm /__admin/health answers 200.
2. Call personalize-runtime GetRecommendations (POST /recommendations) with a userId only — omit both campaignArn and recommenderArn.
3. Observe: HTTP 400, __type InvalidInputException, message `Either campaignArn or recommenderArn is required.`
4. Expected: HTTP 400, InvalidInputException, message exactly `campaignArn or recommenderArn required.`
5. Replay fixture personalize-runtime-recommendations-input-validation-errors, steps task#2 and task#3, to confirm.
