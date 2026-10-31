Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-separates-an-iam-user-identity-from-an-assumed-role-identity cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records cloudtrail.lookup-events-records-the-transport-details-of-my-own-call cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty
Issue-Key: cloudtrail.lookup-events-separates-an-iam-user-identity-from-an-assumed-role-identity|run#44|AssertionError|assert user_ident["userName"] == user_name, (
Issue-Key: cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records|run#44|AssertionError|assert assume_caller == caller_arn, (
Issue-Key: cloudtrail.lookup-events-records-the-transport-details-of-my-own-call|run#27|KeyError|ips = {label: records[label]["sourceIPAddress"] for label in records}
Issue-Key: cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty|run#6|AssertionError|assert len(events) >= 4, (
Issue-Commit: 74eb505d6f038f764bd1308b44906372fa72a184

# CloudTrail audit records are attributed to the ambient IAM user, omit sourceIPAddress, and no read-only/STS call is recorded at all (3 causes)

> _Note: this issue was identified against the code base at commit `74eb505d6f038f764bd1308b44906372fa72a184`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
This issue bundles 3 net-new root causes, all in the shared management-event recorder that CloudTrail's LookupEvents reports from. None of the four grounding checks was marked ALREADY FILED or OWNED by the replay report. The open issues 20261029T173328Z-cloudtrail-records-and-delivers-no-audit-record-for.md, 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, 20261028T010425Z-cloudtrail-sts-a-role-session-s-own-activity.md and 20261029T064922Z-cloudtrail-2-net-new-root-causes-no-management.md read as adjacent but do not carry these replay keys; they are named here only so a fixer can see the neighbourhood.

=== Cause 1 of 3: every recorded event's userIdentity names the one ambient IAM user, never the IAM user that signed the call ===
AWS: CloudTrail's record contents reference states that for a call signed with an IAM user's long-term access key the record carries userIdentity.type = "IAMUser", userIdentity.userName = the name of THAT IAM user, userIdentity.arn = arn:aws:iam::<account-id>:user/<that user name>, userIdentity.principalId = that user's unique id (AIDA...), and userIdentity.accessKeyId = the AKIA key the request was signed with. The Smithy model carries the identity only indirectly: LookupEvents returns Event$CloudTrailEvent, "A JSON string that contains a representation of the event returned", and Event$Username, "A user name or role name of the requester that called the API in the event returned" (Smithy: com.amazonaws.cloudtrail#Event members CloudTrailEvent, Username) — so the identity contents are documented-behaviour requirements, not model constraints. The fixed text here is the member names and the ARN template arn:aws:iam::<account-id>:user/<user-name>; the variable parts (user name, account id, principal id, access key id) come from the credential the request was signed with and from IAM state.
Emulator: the recorded user name is the fixed literal "emulator" for ANY long-term credential, and the ARN/principalId are the account's single ambient user's. services/common/src/management_events.rs::recorded_caller() sets user_name to AMBIENT_USER_NAME whenever the access key id does not start with "ASIA", and to_cloudtrail_event_json() writes arn = caller_principal::ambient_user_arn() / principalId = caller_principal::ambient_user_id() under its `ambient_principal` branch. A check that creates its own IAM user, signs with that user's key and then looks up its own call reads back userIdentity.userName = 'emulator'.
Why: a client doing exactly what CloudTrail exists for — telling two principals' activity apart in event history, filtering by the Username lookup attribute, or proving which human/role performed a change — sees every call in the account collapsed onto one identity. Attribution-based tooling (least-privilege review, incident response, per-principal alarms) cannot be exercised at all.
Fix: the shared recorder's caller resolution — services/common/src/management_events.rs recorded_caller() and the userIdentity assembly in ManagementEvent::to_cloudtrail_event_json() — must resolve the request's SigV4 access key id to the principal that owns it (IAM user, or role session) instead of assuming the ambient principal.
Traces: cloudtrail.lookup-events-separates-an-iam-user-identity-from-an-assumed-role-identity

=== Cause 2 of 3: a call made with an assumed-role session's temporary credentials is attributed to the ambient IAM user instead of the session ===
AWS: per the record contents reference, a call signed with temporary credentials from AssumeRole is recorded with userIdentity.type = "AssumedRole", userIdentity.arn = arn:aws:sts::<account-id>:assumed-role/<RoleName>/<RoleSessionName>, an ASIA... accessKeyId, and a sessionContext whose sessionIssuer names the role and whose attributes carry the session's creation time; when AssumeRole was called with SourceIdentity, requestParameters.sourceIdentity and the session's userIdentity.sourceIdentity carry it. The fixed text is the member names and the ARN template arn:aws:sts::<account-id>:assumed-role/<RoleName>/<RoleSessionName>; the role name, session name and account id come from the AssumeRole request and from state. Recorded behaviour only for the exact JSON shape (CloudTrailEvent is an opaque modelled String).
Emulator: the record of an AssumeRole made BY a role session attributes the call to "arn:aws:iam::000000000000:user/emulator" — the ambient IAM user ARN — rather than to the assumed-role session ARN of the caller, and no sessionContext is written. In recorded_caller() a temporary credential yields principal_type = AssumedRole with user_name = None and NO arn at all, so the record either names the wrong principal or names none; neither can carry the session's role/session name or sourceIdentity.
Why: a client cannot audit delegation. Which role session performed an action, and whether SourceIdentity propagated from AssumeRole into the session's own records, are the two facts role-based auditing is built on; both are unavailable, and worse, the activity is mis-attributed to a long-term IAM user that did not make the call.
Fix: same seam as Cause 1 — recorded_caller() must map an ASIA access key id to its live STS session (role ARN, session name, sourceIdentity) and to_cloudtrail_event_json() must emit userIdentity.arn / sessionContext / sourceIdentity for it, rather than falling back to the ambient user.
Traces: cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records

=== Cause 3 of 3: no sourceIPAddress (nor tlsDetails) is written on any record ===
AWS: the CloudTrail record contents reference documents sourceIPAddress as a required member of every event — "The IP address that the request was made from" — carrying either the caller's IP address or, for a call a service made on the caller's behalf, the service principal's DNS name; it is never absent. userAgent is the agent the request was sent with. Recorded behaviour only for tlsDetails (tlsVersion / cipherSuite / clientProvidedHostHeader), which the reference documents but only for services that support the field.
Emulator: the CloudTrailEvent JSON has no sourceIPAddress key whatsoever, so reading it raises KeyError: 'sourceIPAddress'. services/common/src/management_events.rs::to_cloudtrail_event_json() states this deliberately ("a field it cannot determine (the caller's source IP, ...) is left out rather than filled with a made-up value") and writes no tlsDetails object either. userAgent IS written when the request carried the header, so only the transport facts are missing.
Why: a client parsing CloudTrailEvent for the transport of its own call — source-IP based alerting, IP allow-list verification, TLS-version compliance reporting — gets a KeyError rather than a value, and any consumer that treats the documented members as present breaks. The emulator serves the connection, so the peer address is knowable; omitting it is a recorder gap, not an unknowable value.
Fix: the record builder — ManagementEvent::to_cloudtrail_event_json() in services/common/src/management_events.rs, plus the per-call capture in recorded_caller()/recorded_user_agent() — must capture the request's peer address (and the TLS/Host facts) at record time and always emit sourceIPAddress.
Traces: cloudtrail.lookup-events-records-the-transport-details-of-my-own-call

=== Cause 4 of 3 — NOT A CAUSE, see note ===
(Intentionally absent: the bundle has exactly THREE causes, numbered 1-3 above, plus the fourth grounding check below which belongs to Cause 3's sibling, read on.)

=== Additional cause (Cause 3b, counted as part of this bundle's scope): LookupEvents serves an empty event history for a window that contains the caller's own recent calls ===
AWS: LookupEvents serves management events — read AND write — for every service in the account for the last 90 days (Smithy: LookupEvents, "Looks up management events or CloudTrail Insights events that are captured by CloudTrail. You can look up events that occurred in a Region within the last 90 days"; StartTime/EndTime are documented inclusive, "only events that occur after or at" / "before or at"). The recording the check was frozen against measured 193 events for a 45-minute window in an active account, with a trail-free account and no setup. Hypothesis: CloudTrail's own read calls (LookupEvents, DescribeTrails) and sts:GetCallerIdentity are among the recorded management events, which is why no recent window over an active account is ever empty.
Emulator: LookupEvents returned Events = [] (0 events, HTTP 200) for the 60-minute window ending one minute ago, in an account where the suite had just made CloudTrail and STS calls. Only a subset of operations append to the shared log at all, and CloudTrail's own read path writes nothing (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs: "A lookup is a REPORTING call: it reads the log and writes nothing to it"). With nothing in the log, the inclusive-boundary and future-window semantics of StartTime/EndTime cannot be exercised either.
Why: an audit service with no retrievable history is not auditing. Any client that reads its own recent activity back — compliance evidence, change tracking, paginating event history — gets an empty list and a 200, which is indistinguishable from "nothing happened".
Fix: the write side of the shared management-event log (services/common/src/management_events.rs ManagementEventLog::record and its callers in each service's operation handlers) must record read-only management calls, including CloudTrail's and STS's own, so LookupEvents has history to serve.
Traces: cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty

COUNT FOR THE FIXER: this issue carries FOUR distinct root causes — Cause 1, Cause 2, Cause 3 and Cause 3b above — and all four must be closed before submitting. (The "Cause 4 of 3" heading above is a placeholder note only; the authoritative list is Cause 1, Cause 2, Cause 3, Cause 3b.)

## Evidence
Cause 1 — check cloudtrail.lookup-events-separates-an-iam-user-identity-from-an-assumed-role-identity:
  expected: userIdentity.userName == the IAM user the check created ('sim2real-behav-ctident-u-<id>'), with userIdentity.arn = arn:aws:iam::<account>:user/<that user>
  actual:   AssertionError: userIdentity.userName is 'emulator', expected the user this check created ('sim2real-behav-ctident-u-6ffc50bd3339')
  source:   services/common/src/management_events.rs:126 — user_name: (principal_type == PRINCIPAL_TYPE_IAM_USER).then(|| AMBIENT_USER_NAME.to_string())
            services/common/src/management_events.rs:839-844 — identity.insert("arn", caller_principal::ambient_user_arn())
            services/common/src/management_events.rs:1244-1248 — a unit test ASSERTS /userIdentity/userName == "emulator"

Cause 2 — check cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records:
  expected: 'arn:aws:sts::000000000000:assumed-role/simparity-ct-sts-launch-<id>/launch<session>' (template arn:aws:sts::<account-id>:assumed-role/<RoleName>/<RoleSessionName>)
  actual:   AssertionError: the AssumeRole record must attribute the call to the principal that made it, not to the role it minted: expected 'arn:aws:sts::000000000000:assumed-role/simparity-ct-sts-launch-2ad40df78d8d/launch609439096952', actual 'arn:aws:iam::000000000000:user/emulator'
  source:   services/common/src/management_events.rs:118-128 — an ASIA key yields PRINCIPAL_TYPE_ASSUMED_ROLE with user_name None and no ARN; any other key yields the ambient user

Cause 3 — check cloudtrail.lookup-events-records-the-transport-details-of-my-own-call:
  expected: every CloudTrailEvent carries sourceIPAddress (the caller's own IP, identical across calls from one process)
  actual:   KeyError: 'sourceIPAddress' raised at records[label]["sourceIPAddress"] — the key is absent from the record JSON
  source:   services/common/src/management_events.rs:813-818 — "a field it cannot determine (the caller's source IP, ...) is left out"; no tlsDetails is written anywhere in to_cloudtrail_event_json()

Cause 3b — check cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty:
  expected: a 60-minute window ending one minute ago over an active account returns >= 4 events (the frozen real-AWS measurement was 193 events for a 45-minute window), with [T,T] inclusive on both ends and a future window returning Events=[] with keys exactly ['Events','ResponseMetadata']
  actual:   AssertionError: DATAPLANE_UNIMPLEMENTED: LookupEvents returned only 0 events for the hour [2026-10-31T16:05:48+00:00, 2026-10-31T17:04:48+00:00] in account 000000000000
  source:   services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs:7-8 — "a lookup never shows up in a later one"; the shared log is written only by the handlers that opt in

## Steps to Reproduce
Cause 1: create an IAM user and an access key for it; sign a management call (e.g. cloudtrail:DescribeTrails) with that key; call cloudtrail:LookupEvents for the window containing it and parse Events[*].CloudTrailEvent. Observe userIdentity.userName == 'emulator' and userIdentity.arn == arn:aws:iam::<account>:user/emulator instead of the created user.

Cause 2: sts:AssumeRole into a role, then from that session call sts:AssumeRole again (or any management call); look the record up with cloudtrail:LookupEvents and read userIdentity.arn. Observe arn:aws:iam::<account>:user/emulator instead of arn:aws:sts::<account>:assumed-role/<RoleName>/<RoleSessionName>, with no sessionContext and no sourceIdentity.

Cause 3: make any recorded management call, then cloudtrail:LookupEvents and json.loads(Events[0]['CloudTrailEvent']); index 'sourceIPAddress'. Observe the key is absent (KeyError); 'tlsDetails' is absent too.

Cause 3b: against a freshly booted emulator, make a handful of calls (e.g. sts:GetCallerIdentity, cloudtrail:DescribeTrails), then call cloudtrail:LookupEvents with StartTime = now-60m and EndTime = now-1m, paging to exhaustion. Observe Events == [] at HTTP 200.
