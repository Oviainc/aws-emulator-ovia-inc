Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-runtime-action-recommendations-validation-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-runtime-action-recommendations-validation-errors
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime|GetActionRecommendations|4xx->4xx|$.message
Issue-Commit: f9d8ea6e8a952f12735f5ffe0890d0f58cbc4f2e

# GetActionRecommendations invents a required userId and refuses before resolving campaignArn

> _Note: this issue was identified against the code base at commit `f9d8ea6e8a952f12735f5ffe0890d0f58cbc4f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActionRecommendations

## Description
Cause 1 of 1 - an absent `userId` is refused as a missing required field, and that refusal preempts resolution of `campaignArn`.

AWS: `userId` is NOT a required member of this operation. Smithy: GetActionRecommendationsRequest members are campaignArn (Arn, @length 0..256, @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), userId (UserID, @length 0..256), numResults (NumResults, @range min 0, default 0), filterArn, filterValues - none carries @required, and userId's documentation is only "The user ID of the user to provide action recommendations for."; modeled errors are InvalidInputException (HTTP 400) and ResourceNotFoundException (HTTP 404). Recorded behavior for the request shape in this step: with a `campaignArn` that resolves to no active campaign and no `userId` at all, the service refuses on the CAMPAIGN, returning HTTP 400 with message "<campaignArn> does not exist or is not active yet." - the fixed text is " does not exist or is not active yet." and the only variable part is the campaign ARN copied verbatim from the request's `campaignArn`. The ordering that follows is the requirement: `campaignArn` is resolved first and an absent `userId` never produces a refusal of its own.

Emulator: the same request is answered HTTP 400 with message "userId is required: it is the user to provide action recommendations for." - a wholly fixed string that quotes neither the request nor any state, and the campaign ARN the request named is never resolved or mentioned. The handler settles "request-only" facts first and treats a missing `userId` as fatal there, so the campaign-resolution step is never reached.

Why: a client that got its campaign reference wrong (a typo, the wrong region/account, or a campaign that was deleted or is still CREATE PENDING) is told the problem is a missing `userId`. It supplies a user id, retries, and only then learns the real cause - and an error-classifying wrapper that keys off the message routes a not-found/not-yet-active condition (which AWS reports as retryable while the campaign activates) into a client-side missing-parameter branch. Conversely a caller that legitimately omits `userId`, which the model permits, is refused outright instead of having its campaign reference evaluated.

Fix: the request-only validation stage of the GetActionRecommendations handler. Drop the invented `userId`-presence refusal and let `campaignArn` resolution against the campaign registry produce the refusal, so an unresolvable campaign is reported as such regardless of whether `userId` was sent.

Traces: personalize-runtime-action-recommendations-validation-errors#task#3. Source: services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_action_recommendations.rs lines 179-183 (the `userId` presence check and its literal message) firing ahead of line 198 (`guard.campaign(campaign_arn)`); the not-found text itself lives in src/state.rs (`Denied::unresolvable`). Note: the open issue 20261023T071630Z-personalize-runtime-cannot-distinguish-its-rejection-reasons-a.md concerns a different pair of causes on this path (a wrong-resource-type ARN reported as not-found, and GetRecommendations' no-target message wording) and does not cover the `userId` requirement or the ordering reported here.

## Evidence
Fixture personalize-runtime-action-recommendations-validation-errors, step task#3, operation GetActionRecommendations. Status matches (400 expected, 400 actual); the response body's $.message diverges.
Expected (recorded): "arn:aws:personalize:us-west-2:000000000000:campaign/sim2real-probe-personalize-runtime-campaign-52625ca3 does not exist or is not active yet." - fixed text " does not exist or is not active yet.", variable part the campaign ARN taken from the request (the account id, region and campaign name in that recorded ARN are per-run values, not part of the requirement).
Actual (emulator): "userId is required: it is the user to provide action recommendations for." - all fixed text; the request's campaignArn appears nowhere.
Error code InvalidInputException in both cases; only the message, and hence the reason reported, differs.

## Steps to Reproduce
1. Boot the emulator and replay fixture personalize-runtime-action-recommendations-validation-errors.
2. At step task#3, call personalize-runtime GetActionRecommendations with campaignArn set to a well-formed own-account campaign ARN that no registered campaign matches, and NO userId member in the request body.
3. Observed: HTTP 400 InvalidInputException with message "userId is required: it is the user to provide action recommendations for."
4. Expected: HTTP 400 InvalidInputException with message "<the campaignArn sent in step 2> does not exist or is not active yet."
5. Repeat the same request with a userId added: the emulator then reports the campaign-not-found message, confirming the userId check is what preempts campaign resolution.
