Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.dashboard-create-update-delete-lifecycle cloudtrail.dashboard-list-and-defaults cloudtrail.dashboard-termination-protection-blocks-delete cloudtrail.dashboard-termination-protection-lifecycle-toggle cloudtrail.resource-policy-document-byte-equality-round-trip cloudtrail.resource-policy-invalid-document-rejected cloudtrail.resource-policy-roundtrip-on-dashboard
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.dashboard-create-update-delete-lifecycle|run#6|AssertionError|assert created["TerminationProtectionEnabled"] is True, created
Issue-Key: cloudtrail.dashboard-list-and-defaults|run#6|AssertionError|assert resp.get("TerminationProtectionEnabled") is True, resp
Issue-Key: cloudtrail.dashboard-termination-protection-blocks-delete|run#6|AssertionError|assert err == "ValidationException", f"got={err!r} want=ValidationException"
Issue-Key: cloudtrail.dashboard-termination-protection-lifecycle-toggle|run#8|AssertionError|assert code == "ValidationException", f"got={code!r} want=ValidationException"
Issue-Key: cloudtrail|PutResourcePolicy|ConflictException
Issue-Key: cloudtrail.resource-policy-roundtrip-on-dashboard|run#6|AssertionError|assert pre == ("ResourcePolicyNotFoundException", "There is no policy attached to the given resource."), (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudTrail dashboard termination protection is dropped and PutResourcePolicy always answers ConflictException

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDashboard

## Description
The dashboard and resource-policy surface diverges in four distinct ways. There are 4 distinct causes in this issue; all 4 must be closed.

Cause 1 of 4 — CreateDashboard ignores TerminationProtectionEnabled.
AWS: Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101.json) CreateDashboardRequest member TerminationProtectionEnabled targets TerminationProtectionEnabled (boolean), and CreateDashboardResponse echoes the same member. A request that sets it to true is echoed as true and the stored dashboard reports true from GetDashboard/ListDashboards.
Emulator: CreateDashboard returns HTTP 200 with 'TerminationProtectionEnabled': False even when the request set it to true, and the subsequent read also reports False.
Why: a client that enables termination protection believes the dashboard is protected; it is not, and the delete guard never engages.
Fix: CreateDashboard's handler must carry TerminationProtectionEnabled from the request into dashboard state and its serializer must echo the stored value.
Traces: cloudtrail.dashboard-create-update-delete-lifecycle, cloudtrail.dashboard-list-and-defaults.

Cause 2 of 4 — the termination-protection delete guard answers NotImplemented instead of a modelled refusal.
AWS: recorded behavior only: deleting a dashboard whose TerminationProtectionEnabled is true is refused with error code ValidationException; clearing the flag then permits the delete. Smithy binds DeleteDashboard errors ConflictException (409), ResourceNotFoundException (400) and UnsupportedOperationException (400).
Emulator: the call is refused with error code 'NotImplemented' and the message 'operation ... is scaffolded but not implemented in this emulator' — i.e. the guard is not reached at all because the operation on the path is a scaffold stub. Related open issues 20261022T230533Z-cloudtrail-deletedashboard-stub.md and 20261022T230533Z-cloudtrail-updatedashboard-stub.md cover the stubs; this cause is the termination-protection semantics those handlers must then implement, and the suite did not mark these two checks as covered by them.
Why: a client cannot tell a protected dashboard from an unimplemented API; NotImplemented is not a modelled CloudTrail error and is not retried or branched on by any SDK.
Fix: the dashboard delete/update handlers must implement the termination-protection guard and return ValidationException.
Traces: cloudtrail.dashboard-termination-protection-blocks-delete, cloudtrail.dashboard-termination-protection-lifecycle-toggle.

Cause 3 of 4 — PutResourcePolicy on a freshly created dashboard always fails ConflictException.
AWS: Smithy binds PutResourcePolicy errors ConflictException (httpError 409), ResourceNotFoundException (400), ResourcePolicyNotValidException (400), ResourceARNNotValidException (400), ResourceTypeNotSupportedException (400). ConflictException means the resource is genuinely in a conflicting state; a dashboard that CreateDashboard has already returned accepts a resource policy, and an invalid policy document is refused with ResourcePolicyNotValidException.
Emulator: every PutResourcePolicy against a just-created dashboard raises ConflictException with message 'Resource <dashboard arn> is not ready for this operation; wait a few minutes and then try the operation again'. Both the byte-equality round-trip and the invalid-document check die on this before reaching their assertions, so neither the success path nor ResourcePolicyNotValidException is reachable.
Why: resource policies on dashboards are completely unusable, and a client cannot distinguish a real conflict from a permanent refusal — the suggested retry never succeeds.
Fix: PutResourcePolicy's handler/state must treat a dashboard that CreateDashboard has returned as ready, and must validate the document and return ResourcePolicyNotValidException for a malformed one.
Traces: cloudtrail.resource-policy-document-byte-equality-round-trip, cloudtrail.resource-policy-invalid-document-rejected.

Cause 4 of 4 — GetResourcePolicy's ResourcePolicyNotFoundException carries a different message.
AWS: recorded behavior only: when no policy is attached, GetResourcePolicy returns code ResourcePolicyNotFoundException with the fixed message 'There is no policy attached to the given resource.'; DeleteResourcePolicy returns the same pair. The code and status match in the emulator; only the message text differs, and the rule producing AWS's text is a fixed string (not request-derived), so this is the lowest-severity cause here.
Emulator: code ResourcePolicyNotFoundException with message 'No resource policy is attached to arn:aws:cloudtrail:us-west-2:000000000000:dashboard/<name>' (example).
Why: a client matching on AWS's documented message, as this check does, does not recognise the absence.
Fix: the resource-policy handlers' error construction should emit AWS's fixed ResourcePolicyNotFoundException message.
Traces: cloudtrail.resource-policy-roundtrip-on-dashboard.

## Evidence
Cause 1: assert created['TerminationProtectionEnabled'] is True failed against {'DashboardArn': 'arn:aws:cloudtrail:us-west-2:000000000000:dashboard/sim2real-behav-dash-2411f38e180f', 'Type': 'CUSTOM', 'Status': 'CREATED', 'Widgets': [], 'TerminationProtectionEnabled': False, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}; and resp.get('TerminationProtectionEnabled') is True failed against {'DashboardArn': ..., 'Name': 'sim2real-behav-dash-698c9818901e', 'Type': 'CUSTOM', 'Widgets': [], 'TagsList': [], 'TerminationProtectionEnabled': False}.
Cause 2: "AssertionError: got='NotImplemented' want=ValidationException" from both checks.
Cause 3: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the PutResourcePolicy operation: Resource arn:aws:cloudtrail:us-west-2:000000000000:dashboard/sim2real-ct-rp-dash-4b38daca1373 is not ready for this operation; wait a few minutes and then try the operation again (same for sim2real-behav-dash-4aec80d2da26).
Cause 4: "expected ResourcePolicyNotFoundException before any PutResourcePolicy, got ('ResourcePolicyNotFoundException', 'No resource policy is attached to arn:aws:cloudtrail:us-west-2:000000000000:dashboard/sim2real-behav-dash-f5064ab2fc1a')" — expected message 'There is no policy attached to the given resource.'

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay the checks in trace_ids.
3. Manually: (a) cloudtrail CreateDashboard with TerminationProtectionEnabled=true and read the echoed/stored value; (b) DeleteDashboard on a protected dashboard -> expect ValidationException, observe NotImplemented; (c) CreateDashboard then immediately PutResourcePolicy with a valid policy -> expect 200, observe ConflictException 409; repeat with a malformed document -> expect ResourcePolicyNotValidException; (d) GetResourcePolicy on a dashboard with no policy and compare the message.
