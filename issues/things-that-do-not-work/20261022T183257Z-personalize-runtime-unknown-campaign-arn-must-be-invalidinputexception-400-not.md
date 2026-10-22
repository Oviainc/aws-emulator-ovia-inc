Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-runtime-action-recommendations-for-user
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-runtime-action-recommendations-for-user personalize-runtime-action-recommendations-validation-errors personalize-runtime-personalized-ranking-rerank-item-list personalize-runtime-personalized-ranking-validation-errors personalize-runtime-recommendations-boundary-default-values personalize-runtime-recommendations-campaign-then-recommender-fallback personalize-runtime-recommendations-related-items-with-promotion-and-filter
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime|GetActionRecommendations|4xx->4xx|$.message
Issue-Key: personalize-runtime|GetActionRecommendations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: personalize-runtime|GetActionRecommendations|4xx->4xx|status
Issue-Key: personalize-runtime|GetPersonalizedRanking|4xx->4xx|$.message
Issue-Key: personalize-runtime|GetPersonalizedRanking|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: personalize-runtime|GetPersonalizedRanking|4xx->4xx|status
Issue-Key: personalize-runtime|GetRecommendations|4xx->4xx|$.message
Issue-Key: personalize-runtime|GetRecommendations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: personalize-runtime|GetRecommendations|4xx->4xx|status
Issue-Commit: e1f0928f42e668c41ff2e6f64e4827860a513abf

# Unknown campaign ARN must be InvalidInputException 400, not ResourceNotFoundException 404

> _Note: this issue was identified against the code base at commit `e1f0928f42e668c41ff2e6f64e4827860a513abf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActionRecommendations

## Description
All three Personalize Runtime inference operations resolve a well-formed `campaignArn` that the inference registry does not hold, and all three answer with the WRONG modeled error: HTTP 404 `ResourceNotFoundException` where AWS answers HTTP 400 `InvalidInputException`. Three causes, one per operation, all rooted in the same lookup-to-error mapping.

Cause 1 of 3: GetActionRecommendations.
AWS: `GetActionRecommendations` models BOTH errors (Smithy: GetActionRecommendations errors InvalidInputException httpError 400, ResourceNotFoundException httpError 404; `campaignArn` -> Arn, @length 0..256, @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$). The recording shows that for a syntactically valid campaign ARN that does not resolve, the data plane uses the INVALID-INPUT channel, not the not-found channel: status 400 with header `x-amzn-errortype: InvalidInputException:http://internal.amazon.com/coral/com.amazonaws.services.personalize.runtime.exceptions/` and a message of the form `<campaignArn> does not exist or is not active yet.` (recorded example: `arn:aws:personalize:us-west-2:000000000000:campaign/sim2real-probe-personalize-runtime-campaign-6a2b2bf8 does not exist or is not active yet.`). Hypothesis: the data plane cannot distinguish "never created" from "created but not yet ACTIVE", so it reports both as invalid input — which is exactly what the recorded message says.
Emulator: status 404, `x-amzn-errortype: ResourceNotFoundException`, message `Resource <arn> does not exist.`
Why: a client cannot tell a transient not-yet-ACTIVE campaign from a permanently absent one. SDK retry/classification logic keys off the error code, and the modeled 400 InvalidInputException and 404 ResourceNotFoundException are different shapes in a generated client, so `except InvalidInputException` / `catch (InvalidInputException)` written against AWS never fires here and the error escapes as an unhandled 404. Fixture personalize-runtime-action-recommendations-for-user#task#1 is a mainline recommendation request, so the whole flow is unreachable, not just an error path.
Fix: the campaign/recommender resolution in the service state and the per-operation lift of its refusal — `crate::state`'s `campaign()` returning `Denied::NotFound` and the `deny()` mapping that turns `Denied::NotFound` into `ResourceNotFoundException`. An unresolvable campaign on the inference path must become `InvalidInputException` with the `does not exist or is not active yet.` wording; keep `ResourceNotFoundException` only where the recordings show it.
Traces: personalize-runtime-action-recommendations-for-user#task#1, personalize-runtime-action-recommendations-validation-errors#task#3.

Cause 2 of 3: GetPersonalizedRanking.
AWS: same model shape (Smithy: GetPersonalizedRanking errors InvalidInputException 400, ResourceNotFoundException 404; `campaignArn` @required). Recorded: status 400, `x-amzn-errortype: InvalidInputException:...personalize.runtime.exceptions/`, message `<campaignArn> does not exist or is not active yet.`
Emulator: status 404, `ResourceNotFoundException`, `Resource <arn> does not exist.`
Why: as cause 1 — the rerank mainline cannot be exercised at all.
Fix: same resolution/mapping seam, reached from the GetPersonalizedRanking handler.
Traces: personalize-runtime-personalized-ranking-rerank-item-list#task#1, personalize-runtime-personalized-ranking-validation-errors#task#2.

Cause 3 of 3: GetRecommendations.
AWS: same model shape (Smithy: GetRecommendations errors InvalidInputException 400, ResourceNotFoundException 404). Recorded: status 400, `x-amzn-errortype: InvalidInputException:...personalize.runtime.exceptions/`, message `<campaignArn> does not exist or is not active yet.`
Emulator: status 404, `ResourceNotFoundException`, `Resource <arn> does not exist.`
Why: as cause 1; this is the primary recommendation API, so boundary/default-value, promotion+filter and campaign-then-recommender-fallback flows are all blocked behind it.
Fix: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs` `resolve()`/`deny()` and the shared state lookup they call.
Traces: personalize-runtime-recommendations-boundary-default-values#task#1, personalize-runtime-recommendations-campaign-then-recommender-fallback#task#1 and #task#2, personalize-runtime-recommendations-related-items-with-promotion-and-filter#task#1.

Note: the ARN, account and region in the recorded messages are environment-specific examples; the requirement is the status, the error code and the message FORM (`<campaignArn> does not exist or is not active yet.`). The one open Personalize issue ("Wire the Personalize control plane to the Personalize Events registry") is about the personalize/personalize-events ingestion registry and does not cover this.

## Evidence
Replay of simparity-wire-aws-v1, 3 clusters covering 8 steps.

GetActionRecommendations (personalize-runtime-action-recommendations-for-user#task#1):
  expected status 400, actual 404
  $headers.x-amzn-errortype: expected 'InvalidInputException:http://internal.amazon.com/coral/com.amazonaws.services.personalize.runtime.exceptions/', got 'ResourceNotFoundException'
  $.message: expected 'arn:aws:personalize:us-west-2:000000000000:campaign/sim2real-probe-personalize-runtime-campaign-6a2b2bf8 does not exist or is not active yet.', got 'Resource arn:...-6a2b2bf8 does not exist.'
  also affects personalize-runtime-action-recommendations-validation-errors#task#3

GetPersonalizedRanking (personalize-runtime-personalized-ranking-rerank-item-list#task#1):
  expected status 400, actual 404; same errortype and message divergence on campaign ...-ba14b0ea
  also affects personalize-runtime-personalized-ranking-validation-errors#task#2

GetRecommendations (personalize-runtime-recommendations-boundary-default-values#task#1):
  expected status 400, actual 404; same errortype and message divergence on campaign ...-fa24faf2
  also affects personalize-runtime-recommendations-campaign-then-recommender-fallback#task#1, #task#2 and personalize-runtime-recommendations-related-items-with-promotion-and-filter#task#1

Model (smithy_query operation=GetRecommendations,GetPersonalizedRanking,GetActionRecommendations): every one of the three binds InvalidInputException (httpError 400) AND ResourceNotFoundException (httpError 404), so answering 400 InvalidInputException is expressible today.

Source: ops/get_recommendations.rs `deny()` maps `Denied::NotFound` -> `not_found()` -> ResourceNotFoundException (404); `resolve()` obtains it from `state.campaign(arn)`. ops/get_action_recommendations.rs line 164 does the same via `guard.campaign(campaign_arn).map_err(deny)`.

## Steps to Reproduce
1. Boot the emulator and POST /recommendations with a campaignArn that matches ^arn:([a-z\d-]+):personalize:.*:.*:.+$ but was never registered, e.g. arn:aws:personalize:us-west-2:000000000000:campaign/does-not-exist.
2. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException. AWS answers HTTP 400 with x-amzn-errortype: InvalidInputException:http://internal.amazon.com/coral/com.amazonaws.services.personalize.runtime.exceptions/ and message '<arn> does not exist or is not active yet.'
3. Repeat against POST /personalize-ranking and POST /action-recommendations with an unregistered campaignArn - both diverge the same way.
4. Replay fixtures personalize-runtime-action-recommendations-for-user (task#1), personalize-runtime-personalized-ranking-rerank-item-list (task#1) and personalize-runtime-recommendations-boundary-default-values (task#1).
