Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.put-dial-request-batch-before-start-rejected connectcampaigns.put-dial-request-batch-concurrent-same-client-token-deduplicated connectcampaigns.put-dial-request-batch-deleted-campaign-not-found connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup connectcampaigns.put-dial-request-batch-empty-list-rejected connectcampaigns.put-dial-request-batch-idempotent-retry connectcampaigns.put-dial-request-batch-invalid-phone-number-rejected connectcampaigns.put-dial-request-batch-nonexistent-campaign-not-found connectcampaigns.put-dial-request-batch-stopped-campaign-rejected connectcampaigns.pause-campaign-not-running-rejected connectcampaigns.pause-campaign-wrong-state-rejected connectcampaigns.stop-campaign-terminal-state connectcampaigns.concurrent-pause-campaign-single-winner connectcampaigns.start-campaign-already-running-is-an-idempotent-no-op connectcampaigns.concurrent-start-campaign-single-winner connectcampaigns.start-campaign-after-stop-rejected connectcampaigns.start-campaign-from-stopped-state-succeeds connectcampaigns.stop-stopped-campaign-twice-is-idempotent connectcampaigns.concurrent-stop-campaign-single-winner
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.put-dial-request-batch-before-start-rejected|run#8|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code}"
Issue-Key: connectcampaigns|PutDialRequestBatch|ValidationException
Issue-Key: connectcampaigns.put-dial-request-batch-deleted-campaign-not-found|run#15|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code: {code}"
Issue-Key: connectcampaigns.put-dial-request-batch-nonexistent-campaign-not-found|run#10|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: connectcampaigns.put-dial-request-batch-stopped-campaign-rejected|run#10|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code} ({message!r})"
Issue-Key: connectcampaigns.pause-campaign-not-running-rejected|run#7|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code!r} ({message!r})"
Issue-Key: connectcampaigns.pause-campaign-wrong-state-rejected|run#7|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code} ({message})"
Issue-Key: connectcampaigns.stop-campaign-terminal-state|run#9|AssertionError|assert code == "ConflictException", (
Issue-Key: connectcampaigns.concurrent-pause-campaign-single-winner|run#15|AssertionError|assert code == "ConflictException", (
Issue-Key: connectcampaigns|StartCampaign|InvalidCampaignStateException
Issue-Key: connectcampaigns.concurrent-start-campaign-single-winner|run#10|AssertionError|assert not failures, f"StartCampaign rejected a racer instead of the idempotent success real AWS gives every one: {results}"
Issue-Key: connectcampaigns.start-campaign-after-stop-rejected|run#10|AssertionError|raise AssertionError(f"StartCampaign on a Stopped campaign SUCCEEDED: {result!r}")
Issue-Key: connectcampaigns.start-campaign-from-stopped-state-succeeds|run#10|AssertionError|raise AssertionError(f"StartCampaign on a Stopped campaign did not raise: {result!r}")
Issue-Key: connectcampaigns|StopCampaign|InvalidCampaignStateException
Issue-Key: connectcampaigns.concurrent-stop-campaign-single-winner|run#12|AssertionError|assert len(successes) == racer_count, (
Issue-Commit: 70ed2d2e7473e9995477fa4e995826b18e438b3d

# ConnectCampaigns (v1) campaign lifecycle and dial plane diverge: PutDialRequestBatch rejects every date-time timestamp, state conflicts use the wrong error class and message, and Start/Stop idempotency is inverted

> _Note: this issue was identified against the code base at commit `70ed2d2e7473e9995477fa4e995826b18e438b3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDialRequestBatch

## Description
Five net-new root causes in the ConnectCampaigns v1 crate (services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice). CONTEXT: the open issue 20261029T232338Z-connectcampaignsv2-campaign-plane-wrong-state-conflict-error-4.md describes the same state-conflict error class for the ConnectCampaignsV2 crate and carries only V2 check ids, so it does not cover the v1 handlers named below; the fixed issue 20261029T172833Z-connectcampaigns-v1-validation-and-not-found-refusals-diverge.md covers v1 constraint/not-found MESSAGES only and none of the causes here. Checks the suite marked ALREADY FILED (ResumeCampaign, GetConnectInstanceConfig, ListTagsForResource, TagResource, UpdateCampaign*, DeleteInstanceOnboardingJob stubs) and the one OWNED by 20261022T145747Z are deliberately not re-filed.

=== Cause 1 of 5: PutDialRequestBatch cannot be called at all — the request deserializer parses expirationTime as an epoch integer, so every well-formed dial batch is refused 400 before any handler runs ===
AWS: Smithy: PutDialRequestBatch (restJson1, PUT /campaigns/{id}/dial-requests); input member dialRequests -> DialRequestList @required @length min 1 max 25; DialRequest.expirationTime -> TimeStamp @required with @timestampFormat("date-time"), i.e. the wire value is an RFC3339 date-time STRING (what every AWS SDK serializes a datetime to for this member), not a number. Modeled errors: ConflictException, InvalidCampaignStateException, ResourceNotFoundException, ThrottlingException (plus service-common AccessDeniedException/InternalServerException/ValidationException). AWS accepts the RFC3339 expirationTime and then applies the operation's own rules (campaign exists, campaign is Running, clientToken idempotency, phoneNumber shape).
Emulator: every call — valid or not — is answered HTTP 400, code `ValidationException`, message "invalid timestamp: failed to parse int", produced by the centralized deserialize step before the handler. The whole data plane of the service is therefore unreachable: the campaign-state, not-found, empty-list and idempotency refusals the operation owes can never be observed. Behind that, the handler itself is still a scaffold stub returning 501 `NotImplemented` (services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/put_dial_request_batch.rs:171), which the deserialization failure masks — so closing this cause means BOTH accepting the date-time wire form AND implementing the operation.
Why: a client cannot submit a single dial request. Worse, the refusal it gets names a validation failure for an input it serialized exactly as the model specifies, so the caller is told its own correct request is malformed and has no way to form an acceptable one.
Fix: the timestamp deserialization for @timestampFormat("date-time") members on this operation's input — the generated PutDialRequestBatch input schema / the shared restJson1 deserializer invoked by the deserialize -> validate -> handle walk — and then the PutDialRequestBatch handler body.
Traces: connectcampaigns.put-dial-request-batch-before-start-rejected, connectcampaigns.put-dial-request-batch-concurrent-same-client-token-deduplicated, connectcampaigns.put-dial-request-batch-deleted-campaign-not-found, connectcampaigns.put-dial-request-batch-duplicate-client-token-dedup, connectcampaigns.put-dial-request-batch-empty-list-rejected, connectcampaigns.put-dial-request-batch-idempotent-retry, connectcampaigns.put-dial-request-batch-invalid-phone-number-rejected, connectcampaigns.put-dial-request-batch-nonexistent-campaign-not-found, connectcampaigns.put-dial-request-batch-stopped-campaign-rejected.

=== Cause 2 of 5: every campaign state-conflict refusal answers InvalidCampaignStateException with hand-written text, where AWS answers ConflictException with "Campaign: <id> is in invalid state: <STATE>" ===
AWS: Smithy: PauseCampaign, StartCampaign, StopCampaign and PutDialRequestBatch all model BOTH ConflictException (HTTP 409) and InvalidCampaignStateException (HTTP 409). Recorded behavior: the error AWS actually returns when a campaign is in a state the requested transition cannot be made from is code `ConflictException` (HTTP 409) with the message "Campaign: <campaignId> is in invalid state: <STATE>" — fixed text "Campaign: " and " is in invalid state: "; variable parts are the campaign id taken from the request path and the campaign's CURRENT state taken from service state, rendered UPPER CASE (INITIALIZED | RUNNING | PAUSED | STOPPED), not the model's mixed-case CampaignState spelling. InvalidCampaignStateException is modeled but is not the error this refusal class uses.
Emulator: answers code `InvalidCampaignStateException` (409) carrying a `state` member and the message "Campaign <id> cannot be paused while it is Initialized" / "... cannot be started while it is <State>" / "... cannot be stopped while it is <State>" — wrong error code, wrong fixed text, and the state in mixed case. Sources: ops/pause_campaign.rs:202-215, ops/start_campaign.rs:207-220, ops/stop_campaign.rs:205-218, all fed by state.rs:223-231 whose TransitionRefused::InvalidState doc names InvalidCampaignStateException as this class's error.
Why: the error CODE is what SDK/botocore error classes and `except` clauses dispatch on. A client written against AWS catches ConflictException and never matches the emulator's exception, so a routine state conflict escapes as an unhandled error; and the message text differs from AWS's for every such refusal.
Fix: the TransitionRefused::InvalidState branch of the Pause/Start/Stop (and, once implemented, PutDialRequestBatch) handlers plus the one shared wording for this refusal class in state.rs — the class must be ConflictException with AWS's sentence and the uppercase state.
Traces: connectcampaigns.pause-campaign-not-running-rejected, connectcampaigns.pause-campaign-wrong-state-rejected, connectcampaigns.stop-campaign-terminal-state, connectcampaigns.concurrent-pause-campaign-single-winner.

=== Cause 3 of 5: StartCampaign is not idempotent — a repeat Start on an already-Running campaign is refused instead of succeeding as a no-op ===
AWS: recorded behavior only (the model states no transition table): StartCampaign on a campaign that is already Running returns HTTP 200 with an empty body and leaves the campaign Running — a repeat or concurrent Start never fails. Smithy: StartCampaign output is Unit, so a success carries no body.
Emulator: the allowed-from set is [Initialized, Stopped] (ops/start_campaign.rs:181-188) and state.rs:148-170 documents that "a request to enter the state it is already in is refused unless `to` is itself in allowed_from", so a second Start answers 409 `InvalidCampaignStateException` "Campaign <id> cannot be started while it is Running". Under 8 concurrent Starts exactly one succeeds and seven are refused.
Why: a caller that retries after a timeout, or any two callers racing to start one campaign, gets a hard failure where AWS gives success — so a safe retry becomes an error path and concurrent orchestration code breaks.
Fix: the StartCampaign handler's allowed-from set / the transition rule in state.rs — Running must be an accepted no-op for Start (200, state unchanged, no audit of a transition that did not happen).
Traces: connectcampaigns.start-campaign-already-running-is-an-idempotent-no-op, connectcampaigns.concurrent-start-campaign-single-winner.

=== Cause 4 of 5: StartCampaign restarts a Stopped campaign — Stopped is terminal on AWS and the restart must be refused ===
AWS: recorded behavior only: Stopped is a terminal run state. StartCampaign on a Stopped campaign is refused with HTTP 409, code `ConflictException`, message "Campaign: <campaignId> is in invalid state: STOPPED" (same template as Cause 2), and the campaign stays Stopped.
Emulator: Stopped is in StartCampaign's allowed-from set (ops/start_campaign.rs:181-188), so the call SUCCEEDS with HTTP 200 and the campaign re-enters Running — an over-permissive transition that AWS does not offer.
Why: the emulator admits a lifecycle edge real AWS forbids. Code that treats Stopped as final (and tests that assert a stopped campaign cannot be revived) passes against the emulator and then breaks, or silently places live calls, against AWS.
Fix: the StartCampaign handler's allowed-from set — drop Stopped and refuse it through the Cause 2 refusal class.
Traces: connectcampaigns.start-campaign-after-stop-rejected, connectcampaigns.start-campaign-from-stopped-state-succeeds.

=== Cause 5 of 5: StopCampaign is not idempotent — a second Stop on an already-Stopped campaign is refused instead of succeeding as a no-op ===
AWS: recorded behavior only: StopCampaign on a campaign already in Stopped returns HTTP 200 with an empty body and leaves it Stopped; concurrent Stops on one Running campaign ALL succeed and the campaign settles in Stopped.
Emulator: StopCampaign's allowed-from set is [Running, Paused] (ops/stop_campaign.rs:179-186), so a re-stop answers 409 `InvalidCampaignStateException` "Campaign <id> cannot be stopped while it is Stopped"; with 8 concurrent Stops one succeeds and seven are refused.
Why: the documented-idempotent shutdown path fails on retry, so a client that re-issues Stop after a timeout, or any two shutdown paths racing, sees a spurious conflict for a campaign that is already in the state it asked for.
Fix: the StopCampaign handler's allowed-from set / the shared transition rule — Stopped must be an accepted no-op for Stop.
Traces: connectcampaigns.stop-stopped-campaign-twice-is-idempotent, connectcampaigns.concurrent-stop-campaign-single-winner.

## Evidence
Cause 1 — PutDialRequestBatch(id=<a Running campaign>, dialRequests=[{clientToken, phoneNumber:'+15555550100', expirationTime: now+10min, attributes:{}}]):
  Expected (AWS): HTTP 200 with successfulRequests holding the one request (failedRequests empty); for a never-started campaign instead HTTP 409 ConflictException "Campaign: <id> is in invalid state: INITIALIZED"; for an unknown/deleted campaign HTTP 404 ResourceNotFoundException.
  Actual (emulator): HTTP 400, code ValidationException, message "invalid timestamp: failed to parse int" — on every variant, before the handler. Behind it, put_dial_request_batch.rs:171 still returns Scaffolded::not_implemented("PutDialRequestBatch").
  Replay: 9 checks, all put-dial-request-batch-* ([error] for the direct calls, [mismatch] where a check asserted a different code, e.g. "expected ResourceNotFoundException, got code='ValidationException' message='invalid timestamp: failed to parse int'").

Cause 2 — PauseCampaign(id=<a campaign still Initialized>):
  Expected (AWS): HTTP 409, code ConflictException, message "Campaign: <id> is in invalid state: INITIALIZED".
  Actual (emulator): code InvalidCampaignStateException, message "Campaign <id> cannot be paused while it is Initialized" (observed ids ed13ef9e1cd40306f22b134bdac21496 / 51a888d092979f3969c8a624d460a502 in that run; only the surrounding text and the uppercase state are the requirement).
  Same for PauseCampaign on a Stopped campaign: expected "Campaign: <id> is in invalid state: STOPPED", got InvalidCampaignStateException "Campaign <id> cannot be paused while it is Stopped".
  Replay: pause-campaign-not-running-rejected [mismatch], pause-campaign-wrong-state-rejected [mismatch], stop-campaign-terminal-state [mismatch], concurrent-pause-campaign-single-winner [mismatch] — 7 of 8 losing racers saw InvalidCampaignStateException where every loser must see ConflictException.

Cause 3 — StartCampaign(id=<campaign already Running>):
  Expected (AWS): HTTP 200, empty body, state still Running; all 8 concurrent racers succeed.
  Actual (emulator): ClientError InvalidCampaignStateException "Campaign 8fc4d85172b71a9d3f499ec1df61339e cannot be started while it is Running"; concurrent run returned [('ok',None)] + 7x ('error','InvalidCampaignStateException').
  Replay: start-campaign-already-running-is-an-idempotent-no-op [error], concurrent-start-campaign-single-winner [mismatch].

Cause 4 — StartCampaign(id=<campaign in Stopped>):
  Expected (AWS): HTTP 409 ConflictException "Campaign: <id> is in invalid state: STOPPED", campaign remains Stopped.
  Actual (emulator): HTTP 200 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '0'}}) and the campaign re-enters Running.
  Replay: start-campaign-after-stop-rejected [mismatch], start-campaign-from-stopped-state-succeeds [mismatch].

Cause 5 — StopCampaign(id=<campaign already Stopped>):
  Expected (AWS): HTTP 200, empty body, state still Stopped; all 8 concurrent Stops on a Running campaign succeed.
  Actual (emulator): ClientError InvalidCampaignStateException "Campaign d0386c36d33c8fda57f7c2e4f698a0bb cannot be stopped while it is Stopped"; concurrent run returned [('ok',None)] + 7x ('error','InvalidCampaignStateException').
  Replay: stop-stopped-campaign-twice-is-idempotent [error], concurrent-stop-campaign-single-winner [mismatch].

## Steps to Reproduce
1. Boot the emulator from this branch and point a botocore/boto3 `connectcampaigns` client at it; onboard a Connect instance (connect.create_instance -> campaigns.start_instance_onboarding_job) and create a campaign with create_campaign (agentlessDialerConfig, a contact flow and a source phone number).
2. Cause 1: `client.put_dial_request_batch(id=campaign_id, dialRequests=[{'clientToken':'tok-1','phoneNumber':'+15555550100','expirationTime':datetime.now(timezone.utc)+timedelta(minutes=10),'attributes':{}}])`. Observe HTTP 400 ValidationException "invalid timestamp: failed to parse int" instead of the operation being served (the SDK sends expirationTime as the RFC3339 string the model's @timestampFormat("date-time") requires). Repeat after starting the campaign — same refusal.
3. Cause 2: on the freshly created (Initialized) campaign call `client.pause_campaign(id=campaign_id)`. Observe InvalidCampaignStateException "Campaign <id> cannot be paused while it is Initialized" instead of ConflictException "Campaign: <id> is in invalid state: INITIALIZED".
4. Cause 3: `client.start_campaign(id=campaign_id)`, wait for GetCampaignState == Running, then call start_campaign again. Observe InvalidCampaignStateException instead of HTTP 200. Run 8 start_campaign calls in a ThreadPoolExecutor: only one succeeds.
5. Cause 4: `client.stop_campaign(id=campaign_id)`, wait for Stopped, then `client.start_campaign(id=campaign_id)`. Observe HTTP 200 and GetCampaignState back to Running, instead of ConflictException "Campaign: <id> is in invalid state: STOPPED" with the campaign left Stopped.
6. Cause 5: with the campaign in Stopped, call `client.stop_campaign(id=campaign_id)` again. Observe InvalidCampaignStateException instead of HTTP 200. Run 8 concurrent stop_campaign calls against a Running campaign: only one succeeds.
7. Re-replay the 19 checks in trace_ids to confirm the fix.
