Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.concurrent-create-deployment-same-target-conflict-tolerant
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.concurrent-create-deployment-same-target-conflict-tolerant|run#13|AssertionError|assert code == "ConflictException", f"undocumented CreateDeployment refusal: {code!r} ({message!r})"
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# GreengrassV2 CreateDeployment refused with non-AWS 'AmbiguousService' code under concurrent calls (shared 'greengrass' endpoint prefix)

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: GreengrassV2 CreateDeployment is refused before any handler runs, with an invented, non-AWS error code 'AmbiguousService', because the emulator's request router keys emulated services by endpoint prefix alone and two registered services (Greengrass 2017-06-07 and GreengrassV2 2020-11-30) both claim the prefix 'greengrass'. Failing check: greengrassv2.concurrent-create-deployment-same-target-conflict-tolerant (5 concurrent CreateDeployment calls against one iot thing target). NOTE: this is the same router-level root cause described in the already-open issue 20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass.md; the replay report did NOT mark this check as owned by that issue (that issue's replay keys do not carry this check's assertion site, `assert code == "ConflictException"` in run), so it is filed here to keep it gated. A fixer closing the router issue should re-replay this check id too.

AWS: Smithy com.amazonaws.greengrassv2#GreengrassV2 (version 2020-11-30, restJson1) binds CreateDeployment to POST /greengrass/v2/deployments with success code 201, returning CreateDeploymentResponse {deploymentId, iotJobId, iotJobArn}. Its only modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), RequestAlreadyInProgressException (400), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). GreengrassV2 shares awsService.endpointPrefix / arnNamespace / sigv4Name 'greengrass' with com.amazonaws.greengrass#Greengrass (2017-06-07, restJson1); AWS serves both API versions from greengrass.<region>.amazonaws.com and disambiguates them by each operation's @http method+uri binding (v2 lives under /greengrass/v2/...), never by refusing the request. For concurrent CreateDeployment calls on one target, AWS either creates a distinct deployment per racer or refuses a racer with ConflictException; the recorded AWS ConflictException message for this class is either the fixed text "This request conflicts with another operation and can't be completed. Try again in a few seconds." or the templated "The targetArn '<targetArn>' is already in progress. The current operation will be canceled," whose fixed text is "The targetArn '" / "' is already in progress. The current operation will be canceled," and whose only variable part is the targetArn taken from the request. 'AmbiguousService' is not a modeled error of either service and is not an AWS error code.

Emulator: every one of the 5 concurrent CreateDeployment calls is refused with Error.Code 'AmbiguousService' and the message "2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one" (the host/prefix token is repeated identically, so the two registrations are not even distinguished from each other in the routing table). The refusal is also classified as retryable, so botocore exhausts its retry budget first ("reached max retries: 3") before surfacing it. No deployment is created, so the check's follow-on assertion (exactly one LATEST_ONLY deployment for the target) can never be reached.

Why: a client cannot create a Greengrass v2 deployment against this emulator at all, and the failure is mis-classified twice over. The error code is one no SDK error handler, exception class or retry policy recognises, so client code that distinguishes ConflictException (retry/back off) from ValidationException (fix the request) from ResourceNotFoundException (target missing) gets none of them; and because the refusal is marked retryable, each call burns four attempts of latency before failing. Concurrency-sensitive client logic that treats a ConflictException as "another racer won" instead sees a hard, unrecognised failure for all racers.

Fix: the request router / service-registry dispatch layer, not any GreengrassV2 operation handler (no handler is reached). Dispatch must be keyed by something that actually identifies the service — the sigv4 credential scope plus the resolved @http method+uri binding (and/or API version) — so two services legitimately sharing an endpointPrefix both route. A genuinely unresolvable request must fall back to a modeled error of the resolved service and must not be classified retryable; 'AmbiguousService' must not be emitted on the wire at all.

Traces: greengrassv2.concurrent-create-deployment-same-target-conflict-tolerant.

## Evidence
Expected (AWS): each of the 5 concurrent CreateDeployment calls returns HTTP 201 with CreateDeploymentResponse.deploymentId, or is refused with ConflictException (409) carrying either "This request conflicts with another operation and can't be completed. Try again in a few seconds." (all fixed text) or "The targetArn '<targetArn>' is already in progress. The current operation will be canceled," (fixed text around the request's targetArn). At least one call succeeds and the target then has exactly one LATEST_ONLY deployment.

Actual (emulator): every racer is refused with Error.Code 'AmbiguousService', message "2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one", returned as a retryable condition (botocore reports "reached max retries: 3"). Replay assertion: check_greengrassv2_concurrent_create_deployment_same_target_conflict_tolerant.py line 54 — AssertionError: undocumented CreateDeployment refusal: 'AmbiguousService' ("2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one").

Model citation: smithy_query operation=CreateDeployment on services/service-com-amazonaws-greengrassv2-greengrassv2/smithy-model/com-amazonaws-greengrassv2-greengrassv2.json -> http {POST /greengrass/v2/deployments, code 201}; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, RequestAlreadyInProgressException 400, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; input targetArn required with pattern ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$.

Aggregate: greengrassv2 scored 0/67 (0%) in this suite; 66 of the 67 failures are marked OWNED by the open router issue above, and this one surfaced the same refusal at a different assertion site.

## Steps to Reproduce
1. Boot the emulator with both the greengrass and greengrassv2 services registered (the default build registers both).
2. Create an iot thing: iot.create_thing(thingName=<unique>) and take its thingArn as targetArn.
3. Point a boto3/botocore 'greengrassv2' client at the emulator endpoint with any credentials/region.
4. Issue 5 concurrent gg.create_deployment(targetArn=targetArn) calls from a thread pool.
5. Observe: all 5 raise ClientError with Error.Code == 'AmbiguousService' after exhausting botocore retries, instead of returning 201/deploymentId or a modeled ConflictException (409). No deployment exists afterwards, so list_deployments(targetArn=..., historyFilter='LATEST_ONLY') is empty.
6. Replay check id greengrassv2.concurrent-create-deployment-same-target-conflict-tolerant to confirm; it passes only once greengrass and greengrassv2 both route on the shared 'greengrass' endpoint prefix.
