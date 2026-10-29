Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions connectcampaignsv2.pause-initialized-campaign-rejected connectcampaignsv2.resume-initialized-campaign-rejected connectcampaignsv2.stop-initialized-campaign-rejected connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected connectcampaignsv2.delete-campaign-removes-resource connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded connectcampaignsv2.create-campaign-nonexistent-instance-not-found connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected connectcampaignsv2.update-campaign-channel-subtype-config-round-trip connectcampaignsv2.update-campaign-schedule-persists
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions|run#17|AssertionError|assert error["Code"] == "ConflictException", f"expected ConflictException, got {error}"
Issue-Key: connectcampaignsv2.pause-initialized-campaign-rejected|run#11|AssertionError|assert err["Code"] == "ConflictException", f"expected ConflictException, got {err['Code']}"
Issue-Key: connectcampaignsv2.resume-initialized-campaign-rejected|run#9|AssertionError|assert err["Code"] == "ConflictException", f"expected ConflictException, got {err['Code']}"
Issue-Key: connectcampaignsv2.stop-initialized-campaign-rejected|run#13|AssertionError|assert err["Code"] == "ConflictException", f"expected ConflictException, got {err['Code']}"
Issue-Key: connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected|run#11|AssertionError|assert err["Code"] == "ConflictException", f"expected ConflictException, got {err['Code']}"
Issue-Key: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected|run#9|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected|run#11|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected|run#14|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: connectcampaignsv2.delete-campaign-removes-resource|run#16|AssertionError|assert err["Message"] == expected_message, (
Issue-Key: connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded|run#11|AssertionError|assert err["Message"] == expected_message, (
Issue-Key: connectcampaignsv2.create-campaign-nonexistent-instance-not-found|_error#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: connectcampaignsv2.update-campaign-channel-subtype-config-round-trip|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: connectcampaignsv2.update-campaign-schedule-persists|run#17|AssertionError|assert desc["schedule"]["startTime"] == expected_start, (
Issue-Commit: eeea329c3c54a54e35df274e207bad0e90c767aa

# ConnectCampaignsV2 campaign plane: wrong state-conflict error, 4 wrong refusal messages, 3 missing refusals and microsecond timestamps

> _Note: this issue was identified against the code base at commit `eeea329c3c54a54e35df274e207bad0e90c767aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PauseCampaign

## Description
TEN causes, all in the ConnectCampaignsV2 crate (service token `connectcampaignsv2`), found by replaying the behavioral suite simparity-behavioral-aws-v1 (10/25 passed). A fixer must close all TEN before submitting; each is an independent code path and fixing one leaves the others diverging.

None of these is covered by the open ConnectCampaignsV2 issues. In particular: 20261029T205136Z-connect-campaigns-connectcampaignsv2-campaign-plane-refusals-diverge-unvalidated-connect-instance.md covers CreateCampaign's un-onboarded-instance check, DescribeCampaign's not-found wording and ListCampaigns' base64 nextToken wording — Cause 5 below is the DeleteCampaign sibling of its Cause 2 but a DIFFERENT, currently-live wording defect (the emulator now answers 'CampaignId not found.' with the id dropped, which that issue does not describe), and Cause 7 below is ListCampaigns' *instanceIdFilter* validation, not its nextToken. 20261028T161536Z-connectcampaignsv2-updatecampaignchannelsubtypeconfig-wrong-behavior.md and 20261029T020917Z-connectcampaignsv2-updatecampaignchannelsubtypeconfig-validation.md are about UpdateCampaignChannelSubtypeConfig storing an empty/unvalidated config; Cause 9 below is DeleteCampaignChannelSubtypeConfig's last-subtype guard. 20261029T183744Z-...-wrong-content-type owns only the Content-Type header.

=== Cause 1 of 10: every campaign state-gate refusal uses InvalidCampaignStateException with the emulator's own wording, where AWS uses ConflictException ===
AWS: PauseCampaign / ResumeCampaign / StopCampaign / StartCampaign / PutOutboundRequestBatch refuse a campaign whose CampaignState does not permit the transition (e.g. Pause/Resume/Stop on an Initialized campaign, PutOutboundRequestBatch on a campaign that is not Running, Start on a terminal Stopped campaign) with **ConflictException**, HTTP 409, message = fixed text `Campaign: ` + the campaign id taken from the request's `{id}` path label + ` is in invalid state: ` + the campaign's current state as the UPPER-CASE spelling (`INITIALIZED`, `PAUSED`, `STOPPED`). Smithy: PauseCampaign/ResumeCampaign/StopCampaign/StartCampaign/PutOutboundRequestBatch all model BOTH `ConflictException` (errorKind client, httpError 409) and `InvalidCampaignStateException` (errorKind client, httpError 409, required members `state` + `message`); the recorded refusals for these state gates are ConflictException, so InvalidCampaignStateException is not the shape AWS selects here.
Emulator: returns `InvalidCampaignStateException` with its own prose, e.g. `Campaign <id> is Paused and takes outbound requests only while Running` for PutOutboundRequestBatch, and `InvalidCampaignStateException` for Pause/Resume/Stop on an Initialized campaign. Status class (409) is right; the error code and the message text are not.
Why: botocore surfaces the code from `x-amzn-errortype`, so a client's `except ConflictException` / `if e.response['Error']['Code'] == 'ConflictException'` branch never fires — the refusal escapes as an unmodelled-for-this-path error and the retry/backoff-then-start logic AWS clients write around state conflicts is dead code. Anything logging or matching the message sees text AWS never produces, and the UPPER-CASE state token that AWS puts in the message (which callers parse to decide whether to start or resume) is absent.
Fix: the shared campaign state-gate check used by the PauseCampaign, ResumeCampaign, StopCampaign, StartCampaign and PutOutboundRequestBatch handlers — it must raise ConflictException (409) with the `Campaign: <id> is in invalid state: <STATE>` message for these transitions, keeping InvalidCampaignStateException for the paths where the recordings actually show it.
Traces: connectcampaignsv2.pause-initialized-campaign-rejected, connectcampaignsv2.resume-initialized-campaign-rejected, connectcampaignsv2.stop-initialized-campaign-rejected, connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected, connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions.

=== Cause 2 of 10: CreateCampaign's pattern ValidationException echoes the value and anchors the pattern ===
AWS: CreateCampaign with a `connectInstanceId` that violates its pattern is refused with ValidationException (Smithy: input member `connectInstanceId` @required, target InstanceId, @length min 1 max 256, @pattern ^[-_.a-zA-Z0-9]+$; operation models ValidationException errorKind client httpError 400). The message is the standard AWS framework form, which for this member does NOT echo the offending value and prints the pattern WITHOUT the ^…$ anchors: fixed text `1 validation error detected: Value at '` + the member path (`connectInstanceId`) + `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` + the pattern body (`[-_.a-zA-Z0-9]+`).
Emulator: returns ValidationException/400 but with `1 validation error detected: Value '<the supplied value>' at 'connectInstanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_.a-zA-Z0-9]+$` — it interpolates the rejected value where AWS omits it, and emits the anchored regex source.
Why: the message is the only machine-readable description of which member failed; tests, SDK-level message assertions and user-facing error surfaces all diverge. Echoing the caller's value also leaks input AWS deliberately suppresses for this member.
Fix: the constraint-violation message formatter in the ConnectCampaignsV2 validation layer — strip the ^/$ anchors when rendering a @pattern, and follow AWS's value-elision rule for this member.
Traces: connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected.

=== Cause 3 of 10: GetCampaignStateBatch reports the failing list element instead of the list member with its composite constraint set ===
AWS: GetCampaignStateBatch with an id in `campaignIds` that violates the element constraints is refused with ValidationException/400, reported against the LIST member and rendering the whole list value, with ALL element constraints listed as a composite: fixed text `1 validation error detected: Value '[` + the comma-joined supplied ids + `]' at 'campaignIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ` + the unanchored pattern body (`[-:/a-zA-Z0-9]+`) + `]`. Smithy: CampaignId @length min 1 max 256, @pattern ^[-:/a-zA-Z0-9]+$.
Emulator: returns `1 validation error detected: Value 'not a valid id!' at 'campaignIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-:/a-zA-Z0-9]+$` — per-element path (`campaignIds.1.member`), scalar value instead of the bracketed list, a single constraint instead of the composite set, anchored pattern.
Why: callers that map the member path back to the offending input field are pointed at a path AWS never emits, and only one of the three constraints is reported, so a caller cannot see the length bounds that also apply.
Fix: list-member constraint reporting in the ConnectCampaignsV2 validation layer for GetCampaignStateBatch — report at the list member with the bracketed list rendering and the composite `Member must satisfy constraint: [...]` set.
Traces: connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected.

=== Cause 4 of 10: PutOutboundRequestBatch's ValidationException body is not wrapped in the JSON envelope AWS returns ===
AWS: PutOutboundRequestBatch with an `outboundRequests[*].clientToken` that violates its pattern is refused with ValidationException/400 whose `message` member is itself a JSON document: fixed text `{ "message": "` + `1 validation error detected: Value '` + the supplied token + `' at '` + the element member path (`outboundRequests.1.member.clientToken`) + `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` + the UNANCHORED pattern body (`[a-zA-Z0-9_\-.]*`) + `" }` — i.e. the framework message nested inside a `{ "message": ... }` envelope, with the single-space padding shown. Smithy: PutOutboundRequestBatch models ValidationException (client, 400); outboundRequests @required, @length min 1 max 25.
Emulator: returns the bare framework message `1 validation error detected: Value 'bad token! not allowed' at 'outboundRequests.1.member.clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_\-.]*$` — no envelope, and the anchored pattern.
Why: this operation's refusal body is double-encoded on AWS; a client that json-decodes `Error.Message` (the only way to get at the inner message on this path) fails on the emulator, and the anchored pattern diverges as in Causes 2 and 3.
Fix: the PutOutboundRequestBatch handler's validation error path — preserve the `{ "message": "…" }` envelope AWS wraps this operation's validation failures in, and drop the regex anchors.
Traces: connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected.

=== Cause 5 of 10: DeleteCampaign's ResourceNotFoundException message drops the campaign id ===
AWS: DeleteCampaign on an id that does not exist returns ResourceNotFoundException (Smithy: DeleteCampaign models ResourceNotFoundException, errorKind client, httpError 404; input `id` @required @httpLabel). The message is fixed text `CampaignId not found ` + the campaign id taken verbatim from the request's `{id}` path label + `.`. (The sibling DescribeCampaign path records the id-less form `CampaignId not found.`, which the emulator already produces — so the two paths genuinely differ and must not be unified.)
Emulator: answers 404 ResourceNotFoundException with `CampaignId not found.` for DeleteCampaign too — the id is missing.
Why: a caller deleting many campaigns concurrently cannot tell WHICH id was missing from the refusal, which is the whole purpose of the id in AWS's message; message assertions diverge.
Fix: the not-found error construction in the DeleteCampaign handler — interpolate the requested id, keeping DescribeCampaign's id-less wording intact.
Traces: connectcampaignsv2.delete-campaign-removes-resource.

=== Cause 6 of 10: GetInstanceOnboardingJobStatus's not-found message is the emulator's own wording ===
AWS: GetInstanceOnboardingJobStatus for a Connect instance with no onboarding record returns ResourceNotFoundException (Smithy: ResourceNotFoundException, client, httpError 404) with fixed text `Instance onboarding record does not exist for aws account id ` + the caller's AWS account id (from the request's credentials) + ` and instance id ` + the `connectInstanceId` path label — no trailing period.
Emulator: answers 404 with `No onboarding job found for Connect instance <instanceId>` — wording AWS never produces, and it omits the account id entirely.
Why: the account id in AWS's message is how a caller distinguishes 'this instance was never onboarded in THIS account' from a cross-account mistake; dropping it, and the different fixed text, break any surfacing or assertion on the refusal.
Fix: the not-found error construction in the GetInstanceOnboardingJobStatus handler — emit the account-scoped wording above.
Traces: connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded.

=== Cause 7 of 10: ListCampaigns does not validate the instanceIdFilter, answering 200 with an empty list where AWS refuses ===
AWS: ListCampaigns validates the filtered Connect instance BEFORE listing anything, so `filters.instanceIdFilter` naming an instance that is not onboarded (here: does not exist) is refused with ValidationException, HTTP 400, message = the fixed literal `Instance is not onboarded` (no variable part, no trailing period). Smithy: ListCampaigns models ValidationException (errorKind client, httpError 400); input member `filters` targets CampaignFilters. This happens regardless of whether other instances in the account are onboarded.
Emulator: answers HTTP 200 with `{"campaignSummaryList": []}` — it treats an unknown instance filter as 'no matches' and performs no onboarding check.
Why: a 200-for-400 in the most dangerous direction: a caller probing whether an instance has campaigns reads 'this instance exists and has none' instead of 'this instance is not onboarded', and silently proceeds. The error branch is never exercised.
Fix: the ListCampaigns handler's filter validation — resolve `filters.instanceIdFilter.value` against the instance-config/onboarding state (the same state GetConnectInstanceConfig reads) before listing, and raise ValidationException with the fixed message when it is absent or not enabled for Campaigns.
Traces: connectcampaignsv2.create-campaign-nonexistent-instance-not-found (its CreateCampaign assertions pass; the failure is on the ListCampaigns probe).

=== Cause 8 of 10: DeleteConnectInstanceConfig succeeds while the instance still owns campaigns ===
AWS: DeleteConnectInstanceConfig with no `campaignDeletionPolicy` is refused while the instance still owns at least one campaign: **InvalidStateException**, HTTP 409, message = the fixed literal `This instance still has campaigns` (no variable part, no trailing period). The config is NOT removed (GetConnectInstanceConfig still succeeds afterwards), and once the blocking campaign is deleted the identical call succeeds. Smithy: DeleteConnectInstanceConfig models InvalidStateException (errorKind client, httpError 409) and takes `campaignDeletionPolicy` as an @httpQuery parameter — the parameter only exists because the un-forced delete is blocked by dependent campaigns.
Emulator: answers HTTP 200 with an empty body and deletes the config while a campaign still references the instance.
Why: the dependency guard is the point of this operation's 409. The emulator leaves orphaned campaigns pointing at a deleted instance config, so every later call against them diverges, and a client that relies on the refusal to know it must pass `campaignDeletionPolicy` (or delete campaigns first) never learns it.
Fix: the DeleteConnectInstanceConfig handler — count campaigns owned by `connectInstanceId` in state and, when non-zero and no `campaignDeletionPolicy` is supplied, raise InvalidStateException (409) with the fixed message and leave the config in place.
Traces: connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected.

=== Cause 9 of 10: DeleteCampaignChannelSubtypeConfig removes the campaign's only configured channel subtype ===
AWS: DeleteCampaignChannelSubtypeConfig is refused when the named `channelSubtype` is the only one the campaign has configured — a campaign must always keep at least one: ValidationException, HTTP 400, message = the fixed literal `At least one channel subtype needs to be configured.` (no variable part). The campaign's `channelSubtypeConfig` is unchanged afterwards. Smithy: DeleteCampaignChannelSubtypeConfig models ValidationException (errorKind client, httpError 400); `channelSubtype` is @required and bound as @httpQuery.
Emulator: answers HTTP 200 with an empty body for `channelSubtype=TELEPHONY` on a campaign configured with telephony only, leaving the campaign with no channel subtype at all.
Why: the emulator produces a record state AWS never allows (a campaign with an empty channelSubtypeConfig), so subsequent Describe/Start/PutOutboundRequestBatch calls on it diverge arbitrarily; the caller's error branch never fires. (Related but distinct: 20261028T161536Z-connectcampaignsv2-updatecampaignchannelsubtypeconfig-wrong-behavior.md reports the UPDATE path storing a config with no channel — this is the DELETE path's missing last-subtype guard.)
Fix: the DeleteCampaignChannelSubtypeConfig handler's validation — if removing the named subtype would leave the campaign's channelSubtypeConfig empty, raise ValidationException (400) with the fixed message and make no state change.
Traces: connectcampaignsv2.update-campaign-channel-subtype-config-round-trip (its UpdateCampaignChannelSubtypeConfig round-trip assertions pass; the failure is the un-refused delete of the last subtype).

=== Cause 10 of 10: Schedule timestamps are echoed at microsecond precision instead of AWS's millisecond precision ===
AWS: Schedule's `startTime`/`endTime` are modelled with @timestampFormat `date-time` (Smithy: Schedule.startTime/endTime @required, target TimeStamp, timestampFormat date-time). The service stores and echoes them at MILLISECOND precision: the sub-millisecond remainder of a submitted value is dropped, so a value sent as …:38.000890Z comes back as …:38.000Z from DescribeCampaign after UpdateCampaignSchedule.
Emulator: echoes the submitted value at full microsecond precision — DescribeCampaign returned a startTime with microsecond=890 where AWS returns microsecond=0 for the same submitted instant.
Why: round-trip equality checks on a schedule a client just wrote fail against the emulator and pass against AWS (and vice versa); anything hashing/comparing the serialized timestamp, or asserting the service's documented ms granularity, diverges. This is a precision rule, not a clock value: the divergence is reproducible for any submitted value with a non-zero sub-millisecond remainder.
Fix: the timestamp serializer/state write for Schedule in the UpdateCampaignSchedule and CreateCampaign handlers — truncate `date-time` timestamps to millisecond precision on store (and on serialize) as the service does.
Traces: connectcampaignsv2.update-campaign-schedule-persists.

## Evidence
Replay of simparity-behavioral-aws-v1: 10/25 passed (40.0%), 14 mismatched + 1 errored, all in connectcampaignsv2. The errored check (connectcampaignsv2.update-campaign-communication-limits-override-round-trip) is marked ALREADY FILED (blocked by unimplemented customerprofiles CreateSegmentDefinition) and is deliberately NOT part of this issue.

Cause 1 — state-gate error code/message:
  connectcampaignsv2.pause-initialized-campaign-rejected: AssertionError: expected ConflictException, got InvalidCampaignStateException (expected message `Campaign: <campaignId> is in invalid state: INITIALIZED`).
  connectcampaignsv2.resume-initialized-campaign-rejected: same — expected ConflictException, got InvalidCampaignStateException.
  connectcampaignsv2.stop-initialized-campaign-rejected: same — expected ConflictException, got InvalidCampaignStateException.
  connectcampaignsv2.put-outbound-request-batch-on-initialized-campaign-rejected: same — expected ConflictException, got InvalidCampaignStateException.
  connectcampaignsv2.campaign-lifecycle-start-pause-resume-stop-transitions: expected ConflictException, got {'Code': 'InvalidCampaignStateException', 'Message': 'Campaign <id> is Paused and takes outbound requests only while Running'}; AWS: ConflictException with `Campaign: <id> is in invalid state: PAUSED` (and `… : STOPPED` for StartCampaign on a terminal campaign). Fixed text = `Campaign: ` / ` is in invalid state: `; variables = the request's {id} label and the current state in UPPER CASE.

Cause 2 — connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected:
  expected "1 validation error detected: Value at 'connectInstanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_.a-zA-Z0-9]+"
  got      "1 validation error detected: Value 'not a valid instance id!' at 'connectInstanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_.a-zA-Z0-9]+$"
  (the quoted input value is this run's value, not part of the requirement; the requirement is that it is OMITTED and the pattern unanchored.)

Cause 3 — connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected:
  expected "1 validation error detected: Value '[<ids>]' at 'campaignIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [-:/a-zA-Z0-9]+]"
  got      "1 validation error detected: Value '<id>' at 'campaignIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-:/a-zA-Z0-9]+$"

Cause 4 — connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected:
  expected '{ "message": "1 validation error detected: Value \'<token>\' at \'outboundRequests.1.member.clientToken\' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_\\-.]*" }'
  got      "1 validation error detected: Value '<token>' at 'outboundRequests.1.member.clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_\\-.]*$"
  (same check's positive control passes: a pattern-valid token on a missing campaign correctly yields ResourceNotFoundException `Campaign: <id> doesn't exist`.)

Cause 5 — connectcampaignsv2.delete-campaign-removes-resource:
  DeleteCampaign on the already-deleted id: expected message 'CampaignId not found <campaignId>.', got 'CampaignId not found.' (status 404 and code ResourceNotFoundException are correct; the DescribeCampaign variant, which AWS words WITHOUT the id, passes).

Cause 6 — connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded:
  expected 'Instance onboarding record does not exist for aws account id <accountId> and instance id <instanceId>'
  got      'No onboarding job found for Connect instance <instanceId>'
  (the sibling GetConnectInstanceConfig refusal `ConnectInstanceConfig not found <instanceId>.` passes.)

Cause 7 — connectcampaignsv2.create-campaign-nonexistent-instance-not-found:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'campaignSummaryList': []} for ListCampaigns(filters={'instanceIdFilter': {'operator': 'Eq', 'value': <nonexistent instance id>}}).
  Expected (AWS): 400 ValidationException, message 'Instance is not onboarded'.

Cause 8 — connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '0', ...}}} for DeleteConnectInstanceConfig(connectInstanceId=<instance owning one campaign>), no campaignDeletionPolicy.
  Expected (AWS): 409 InvalidStateException, message 'This instance still has campaigns', config left intact.

Cause 9 — connectcampaignsv2.update-campaign-channel-subtype-config-round-trip:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '0', ...}}} for DeleteCampaignChannelSubtypeConfig(id=<campaign>, channelSubtype='TELEPHONY') on a telephony-only campaign.
  Expected (AWS): 400 ValidationException, message 'At least one channel subtype needs to be configured.', channelSubtypeConfig unchanged.

Cause 10 — connectcampaignsv2.update-campaign-schedule-persists:
  expected startTime datetime(…, 23, 47, 38, 0, tzinfo=utc)  (submitted value truncated to millisecond)
  got      datetime(…, 23, 47, 38, 890, tzinfo=tzlocal())     (submitted microseconds preserved)
  The instant is the same; the divergence is the precision rule (AWS drops the sub-millisecond remainder), not the clock value.

## Steps to Reproduce
1. Boot the emulator from mainline and point a botocore `connectcampaignsv2` client at its data listener. Onboard a Connect instance (StartInstanceOnboardingJob) and create a telephony campaign where a cause needs one.
2. Cause 1: on a freshly created (Initialized) campaign call PauseCampaign, ResumeCampaign, StopCampaign and PutOutboundRequestBatch. Each must be refused 409 ConflictException with `Campaign: <id> is in invalid state: INITIALIZED`; the emulator answers InvalidCampaignStateException. Then Start -> Pause the campaign and call PutOutboundRequestBatch: AWS gives ConflictException `… is in invalid state: PAUSED`, the emulator gives InvalidCampaignStateException with its own prose. Stop the campaign and call StartCampaign: AWS gives ConflictException `… is in invalid state: STOPPED`. Replay connectcampaignsv2.pause-initialized-campaign-rejected, .resume-initialized-campaign-rejected, .stop-initialized-campaign-rejected, .put-outbound-request-batch-on-initialized-campaign-rejected, .campaign-lifecycle-start-pause-resume-stop-transitions.
3. Cause 2: CreateCampaign with connectInstanceId='not a valid instance id!' — compare the ValidationException message against the unanchored, value-elided AWS form. Replay connectcampaignsv2.create-campaign-invalid-instance-id-pattern-rejected.
4. Cause 3: GetCampaignStateBatch(campaignIds=['not a valid id!']) — compare against the list-member composite-constraint AWS form. Replay connectcampaignsv2.get-campaign-state-batch-invalid-id-pattern-rejected.
5. Cause 4: PutOutboundRequestBatch with outboundRequests[0].clientToken='bad token! not allowed' — the message must be the `{ "message": "…" }` envelope with the unanchored pattern. Replay connectcampaignsv2.put-outbound-request-batch-invalid-client-token-pattern-rejected.
6. Cause 5: DeleteCampaign twice on the same id; the second must be 404 'CampaignId not found <id>.' (DescribeCampaign on the same id stays 'CampaignId not found.'). Replay connectcampaignsv2.delete-campaign-removes-resource.
7. Cause 6: GetInstanceOnboardingJobStatus on a Connect instance that was never onboarded; expect 404 'Instance onboarding record does not exist for aws account id <acct> and instance id <id>'. Replay connectcampaignsv2.instance-onboarding-job-transitions-to-succeeded.
8. Cause 7: ListCampaigns(filters={'instanceIdFilter': {'operator':'Eq','value': str(uuid4())}}) while another instance IS onboarded; expect 400 ValidationException 'Instance is not onboarded', not 200 with an empty campaignSummaryList. Replay connectcampaignsv2.create-campaign-nonexistent-instance-not-found.
9. Cause 8: with one campaign on the instance, call DeleteConnectInstanceConfig(connectInstanceId=<id>) and omit campaignDeletionPolicy; expect 409 InvalidStateException 'This instance still has campaigns', then GetConnectInstanceConfig still succeeding; after DeleteCampaign the same call must succeed. Replay connectcampaignsv2.delete-connect-instance-config-blocked-by-active-campaign-rejected.
10. Cause 9: on a telephony-only campaign call DeleteCampaignChannelSubtypeConfig(id=<id>, channelSubtype='TELEPHONY'); expect 400 ValidationException 'At least one channel subtype needs to be configured.' and an unchanged channelSubtypeConfig. Replay connectcampaignsv2.update-campaign-channel-subtype-config-round-trip.
11. Cause 10: UpdateCampaignSchedule with a startTime/endTime whose microsecond component is not a whole number of milliseconds (e.g. microsecond=890), then DescribeCampaign; the returned schedule must have the sub-millisecond remainder dropped. Replay connectcampaignsv2.update-campaign-schedule-persists.
12. Re-replay all 14 check ids in Issue-Trace-Ids; all must pass on error code, HTTP status, message text and the state/representation effects above.
