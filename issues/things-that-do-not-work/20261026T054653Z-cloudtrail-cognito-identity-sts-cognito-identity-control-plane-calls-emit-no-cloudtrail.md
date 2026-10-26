Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume
Issue-Key: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity|run#30|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume|run#29|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# Cognito Identity control-plane calls emit no CloudTrail management event, so LookupEvents never surfaces CreateIdentityPool/DescribeIdentityPool and the Cognito→STS audit joins are unobservable

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentityPool

## Description
ONE root cause (cause 1 of 1).

Cause 1 of 1: Amazon Cognito Identity control-plane operations are never written to the account's CloudTrail management-event log, so CloudTrail LookupEvents can never return a record for them.

AWS: CloudTrail records Amazon Cognito Identity control-plane API calls as management events with eventSource `cognito-identity.amazonaws.com`, and CloudTrail's LookupEvents returns them in event history. Smithy (cloudtrail): LookupEvents takes `LookupAttributes` (LookupAttributesList) and `StartTime`/`EndTime` (Date) and returns `Events` (EventsList), each carrying a `CloudTrailEvent` JSON document; it is `@paginated` on NextToken/MaxResults and models no error for "this event source is not recorded" — an unrecorded call is simply absent, so absence is indistinguishable from a dead index. For `CreateIdentityPool` the record's `requestParameters.identityPoolName` is the name the caller sent, and for `DescribeIdentityPool` `requestParameters.identityPoolId` is the pool id; recorded behavior only for the record field layout (the CloudTrail Smithy model does not model the CloudTrailEvent document's members). Both checks use their OWN CreateIdentityPool record as the index-liveness calibration marker, and the second additionally uses a post-act DescribeIdentityPool record as a freeze check, before asserting anything about the STS leg.

Emulator: `CreateIdentityPool::handle` (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/create_identity_pool.rs) authorizes, mints the pool id, inserts into `store.identity_pools` and returns the output — it never writes a management-event record, unlike `GetCredentialsForIdentity`, which in the same crate does call `crate::state::record_vend` and `crate::state::record_vended_assume`. So the recording seam exists in this service and the control-plane operations do not use it. Consequence at replay: both checks poll LookupEvents with `LookupAttributes=[{AttributeKey: EventName, AttributeValue: CreateIdentityPool}]` every 10 s against a 600 s ceiling, the marker never appears, and the harness kills the check first — both report `Check timed out after 60.0s` rather than reaching any downstream assertion. The STS `AssumeRoleWithWebIdentity` leg (eventSource `sts.amazonaws.com`, userIdentity.type `WebIdentityUser`, requestParameters.roleSessionName `CognitoIdentityCredentials`) is therefore never even evaluated, so it may be defective too and is masked by this cause.

Why: a client cannot audit identity-pool administration at all. Every security/compliance consumer that reads event history to answer "who created or reconfigured this identity pool, and when" gets an empty result that is indistinguishable from "nobody did it", and any pipeline that uses its own control-plane write as a liveness barrier before judging a later event's absence (the standard way to avoid a false negative against an eventually-consistent index) can never arm that barrier.

Fix: the Cognito Identity control-plane handlers' management-event recording — `CreateIdentityPool`, and the same omission in the sibling control-plane ops the checks touch (`DescribeIdentityPool`, `SetIdentityPoolRoles`, `GetIdentityPoolRoles`, `DeleteIdentityPool`, `GetOpenIdTokenForDeveloperIdentity`) — must write a management-event record with eventSource `cognito-identity.amazonaws.com`, the operation's own eventName, and `requestParameters` echoing the request's members, through the same engine-level recording seam `GetCredentialsForIdentity` already uses. CloudTrail's LookupEvents reader needs no change if the records are written there.

Traces: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity, cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume. Related open issues, neither of which carries these two checks' replay key for THIS cause: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md records the same class of omission for other event sources, and 20261024T065715Z-cloudtrail-cognito-identity-sts-unsigned-cognito-identity-requests-are-answered-unknownservice-so.md named these two checks for a DIFFERENT, earlier failure mode (the unsigned call answered `UnknownService`); GetCredentialsForIdentity is now implemented and these checks no longer fail that way, they fail on the missing audit record.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (both verdicts: mismatch):

- cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity: "Check timed out after 60.0s". The check is blocked in its calibration loop, whose own failure text states the requirement: "CloudTrail event history never surfaced this run's own CreateIdentityPool for <pool name> within 600s (last scan: <N> page(s), <N> CreateIdentityPool events, none ours); nothing downstream can be judged". Fixed text is the sentence; the variable parts are the pool name created by the request, the page count and the event count from the emulator's state.
- cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume: "Check timed out after 60.0s", blocked in the same CreateIdentityPool calibration loop before it can arm its differential barrier.

Expected: cloudtrail LookupEvents with LookupAttributes EventName=CreateIdentityPool over the run's window returns an Events entry whose CloudTrailEvent has eventSource "cognito-identity.amazonaws.com", eventName "CreateIdentityPool" and requestParameters.identityPoolName equal to the IdentityPoolName that was sent; likewise EventName=DescribeIdentityPool returns a record whose requestParameters.identityPoolId equals the pool id and which carries no errorCode.
Actual: LookupEvents returns no record for either event name, for any pool, for the whole window — the Cognito Identity control plane writes nothing to event history.

Code evidence: services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/create_identity_pool.rs lines 189-271 — handle() performs authorization, the store-full check, id minting and the insert, and returns, with no management-event record written; compare services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs lines 202-217 and 396-411, where the same crate does record its event via crate::state::record_vend / crate::state::record_vended_assume.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay the two check ids in trace_ids; both currently die on the 60 s harness timeout while polling for the CreateIdentityPool marker.
3. Manual repro: call cognito-identity CreateIdentityPool (IdentityPoolName=<name>, AllowUnauthenticatedIdentities=false, DeveloperProviderName=<provider>), then DescribeIdentityPool on the returned IdentityPoolId. Now call cloudtrail LookupEvents with StartTime a few minutes in the past, EndTime a few minutes in the future and LookupAttributes=[{AttributeKey:"EventName",AttributeValue:"CreateIdentityPool"}], paging to exhaustion. The emulator returns an empty Events list; AWS returns a record with eventSource "cognito-identity.amazonaws.com" and requestParameters.identityPoolName equal to <name>. Repeat with AttributeValue "DescribeIdentityPool" for the same result.
4. Once records are written, the downstream STS leg (EventName=AssumeRoleWithWebIdentity, eventSource sts.amazonaws.com, userIdentity.type WebIdentityUser, requestParameters.roleSessionName "CognitoIdentityCredentials", responseElements.credentials.accessKeyId equal to the key GetCredentialsForIdentity returned) becomes assertable and must be verified as part of closing this issue.
