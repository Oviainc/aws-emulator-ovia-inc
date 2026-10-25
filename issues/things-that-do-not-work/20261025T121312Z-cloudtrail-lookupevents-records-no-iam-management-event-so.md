Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record
Issue-Key: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record|run#32|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 2e55e3f174321d59189949bdc160ce3c5e1a8b8d

# cloudtrail: LookupEvents records no IAM management event, so the CreateAccessKey / CreateLoginProfile secret-omission contract can never be observed

> _Note: this issue was identified against the code base at commit `2e55e3f174321d59189949bdc160ce3c5e1a8b8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One net-new root cause (Cause 1 of 1) in the bootstrapped `cloudtrail` service, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: no IAM management event is ever recorded in event history, so a client can never read back the record whose defining property is that it OMITS the credential material the call returned.

AWS: every management call of every service is recorded in the Region's event history and readable through LookupEvents (Smithy com.amazonaws.cloudtrail#LookupEvents, input LookupAttributes/StartTime/EndTime/EventCategory/MaxResults/NextToken, output Events with CloudTrailEvent; the operation's own documentation lists the supported lookup attributes — Event ID, Event name, Event source, Read only, Resource name, Resource type, User name, access key). IAM's events are filed in the partition leader Region, us-east-1. For the two calls this check is about, the recorded shape is measured behavior, not a guess: CreateAccessKey's `responseElements` is {"accessKey": {"userName", "accessKeyId", "status", "createDate"}} — the `secretAccessKey` member is OMITTED ENTIRELY, not replaced with a placeholder such as "HIDDEN_DUE_TO_SECURITY_REASONS" that some other services use — even though the model's AccessKey shape declares SecretAccessKey required and the API response carries it; and CreateLoginProfile's `requestParameters` is {"userName", "passwordResetRequired"} with the `password` member likewise omitted entirely. Neither the secret nor the password appears anywhere in either serialised record.

Emulator: event history holds nothing for these calls. LookupEvents itself is served, out of the shared log (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs reads `service_common::management_events` through `crate::state::management_event_log`), but that log is only ever written by CloudTrail's OWN mutating handlers — src/state.rs documents `record_management_event` as "reached from each mutating handler" of this service. No iam CreateAccessKey or CreateLoginProfile is recorded, so the check issues its two IAM calls and then polls LookupEvents in us-east-1 under its own 255 s budget (`_INDEX_BUDGET_S = 255.0`, `_POLL_INTERVAL_S = 10.0`, `_PAGE_CEILING = 40`) and never sees a single matching record; it is still polling when the harness's 60 s cap kills it, so it never reaches its field assertions. Status and error code do not diverge — LookupEvents answers 200 with `Events: []`.

Why: this is the one check in the suite that proves a NEGATIVE about an audit record — that the long-term secret and the console password were never persisted into the trail. An empty event history makes that claim vacuous: a client auditing its own emulator for credential leakage, or building a compliance control on "no secret material in CloudTrail", gets a pass it has not earned, and cannot distinguish "nothing was recorded" from "nothing happened". Every detective control over IAM identity changes (who minted this key, who set this login profile) reads an empty trail.

Fix: the shared management-event recording seam — `service_common::management_events`, the one CloudTrail's own src/state.rs::record_management_event already writes to — must be driven from the IAM service's served operations as well, populating eventSource/eventName/readOnly/requestParameters/responseElements/requestID, and applying the per-member omission rule so CreateAccessKey's responseElements carries no `secretAccessKey` key and CreateLoginProfile's requestParameters carries no `password` key (omitted, not redacted to a placeholder). IAM's records must be filed under the partition leader Region so a us-east-1 LookupEvents finds them.

Traces: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record. The same underlying gap is already filed as Cause 1 of the open issue 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, but that issue's replay set names cloudtrail__iam.obs-iam-1-read-only-classification-and-response-elements-follow-the-operation and not this check; this one is a separate recorded unit (it grades the credential-OMISSION rule, which no other check in that set covers) and the replay report marked it neither ALREADY FILED nor OWNED, so it is recorded here.

## Evidence
Replay output:
  [mismatch] cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record
  Error: Check timed out after 60.0s

The check's own header records what it measured against real AWS and is therefore what the emulator must produce:
  "CreateAccessKey's responseElements is {\"accessKey\": {\"userName\", \"accessKeyId\", \"status\", \"createDate\"}} -- the secretAccessKey member is OMITTED ENTIRELY. It is NOT replaced with a placeholder such as \"HIDDEN_DUE_TO_SECURITY_REASONS\" ..."
  "CreateLoginProfile's requestParameters is {\"userName\", \"passwordResetRequired\"} -- the password member is likewise OMITTED ENTIRELY, not redacted to a placeholder."
  "Neither the secret nor the password appears anywhere in either serialised record."
Its waits are a LookupEvents poll, not a sleep on a resource: `_LEADER_REGION = "us-east-1"`, `_INDEX_BUDGET_S = 255.0`, `_POLL_INTERVAL_S = 10.0`, `_PAGE_CEILING = 40`. 255 s of polling against a 60 s harness cap is why the verdict is a timeout rather than an assertion: the record never appears, so the field assertions are never reached.

Corroborating code in this checkout: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs serves pages out of `service_common::management_events` via `crate::state::management_event_log`, and src/state.rs states that log is written by `record_management_event`, "reached from each mutating handler" of THIS service only — no IAM handler writes it.

Expected (AWS): LookupEvents in us-east-1 over a window covering the two calls returns one record each, with CreateAccessKey's responseElements lacking `secretAccessKey` and CreateLoginProfile's requestParameters lacking `password`. Actual (emulator): HTTP 200 with `Events: []` on every page, forever.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudtrail and iam are both bootstrapped and serving).
2. iam CreateUser, then CreateAccessKey for that user (the response carries SecretAccessKey), then CreateLoginProfile for the same user with a Password and PasswordResetRequired=true.
3. cloudtrail LookupEvents against us-east-1 with StartTime set just before step 2 and LookupAttributes=[{AttributeKey: "EventName", AttributeValue: "CreateAccessKey"}], then again with "CreateLoginProfile".
4. Expect one Event per call. Parse each `CloudTrailEvent`: CreateAccessKey's responseElements must be {"accessKey": {"userName", "accessKeyId", "status", "createDate"}} with NO `secretAccessKey` key at all, and CreateLoginProfile's requestParameters must be {"userName", "passwordResetRequired"} with NO `password` key at all; the secret and the password must appear nowhere in either serialised record.
5. Observe HTTP 200 with `Events: []` for both lookups, on every page and for as long as you poll.
6. Equivalently: re-replay cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record.
