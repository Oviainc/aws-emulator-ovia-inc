Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.resource-policy-enforced-on-put-rum-events
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.resource-policy-enforced-on-put-rum-events|run#16|AssertionError|assert code_before[1] == denied, f"expected message {denied!r}, got {code_before[1]!r}"
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# PutRumEvents' AccessDeniedException names the app monitor ARN instead of the request's Id and omits "identity-based" from the implicit-deny reason clause

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRumEvents

## Description
This issue carries TWO net-new root causes in the RUM data-plane authorization refusal rendered by PutRumEvents. Both must be closed before submitting; each is numbered and carries its own failing check ids.

Note on existing issues: the open issue 20261029T192752Z-rum-app-monitor-data-plane-resource-policy-tagging.md describes the same two defects as its Cause 3 (denial reason clause) and Cause 4 (resource slot) grounded in other checks (rum.put-rum-events-missing-permission-denied, rum.e2e-resource-policy-access-lifecycle, rum.put-rum-metrics-destination-access-denied). The failing check below was NOT marked OWNED by it, so its replay key is not carried by that issue and it is filed here; a fixer closing that issue's Causes 3/4 should re-replay this check too. Related but distinct: 20261029T220445Z-rum-batchdeleterummetricdefinitions-accessdeniedexception-says-because-no-policy-allows-instead.md reports the same missing "identity-based " literal on BatchDeleteRumMetricDefinitions, which indicates the wording is produced by one shared helper; this issue is the PutRumEvents instance of it plus the resource-slot defect that is specific to PutRumEvents.

Cause 1 of 2: the implicit-deny reason clause is the bare "because no policy allows ..." instead of AWS's "because no identity-based policy allows ...".
AWS: Smithy (services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json) models AccessDeniedException on PutRumEvents (errorKind client, httpError 403; POST /appmonitors/{Id}/, dataplane host prefix). The model does not state the message text, so recorded behavior only: a principal with NO identity-based policy granting the action, and no resource policy on the app monitor granting it, is refused with code AccessDeniedException (403) and the standard IAM implicit-deny sentence. Its FIXED text is "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action"; the VARIABLE parts are the caller's principal ARN taken from the signing credentials (for an assumed role, arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name>), the resource (see Cause 2) and the action name rum:PutRumEvents, which appears twice. The reason clause is what identifies the class of denial: an implicit deny with no identity-based policy in play says "no identity-based policy allows" — not the bare "no policy allows" phrasing, and not an explicit-deny clause.
Emulator: the status (403), the code (AccessDeniedException) and the leading text are correct, and the call is correctly refused, but the reason clause drops the literal "identity-based ": it answers "... because no policy allows the rum:PutRumEvents action".
Why: the reason clause is the only machine-readable discriminator a client has between an implicit deny from a missing identity-based policy and the other denial classes (explicit deny in an identity-based policy, in a resource-based policy, an SCP, a permissions boundary or a session policy). Permission-diagnostics tooling and access-review automation classify the emulator's denial as an unknown reason or fail outright, and a runbook that tells an operator to attach an identity-based policy is never triggered.
Fix: the shared IAM-denial wording helper used by the RUM handlers' authorization path (the service state's authorization refusal construction) must select the "no identity-based policy allows the <action> action" clause for an implicit deny with no identity-based policy, for every operation that renders this sentence.
Traces: rum.resource-policy-enforced-on-put-rum-events.

Cause 2 of 2: the "on resource: " slot is filled with the derived app monitor ARN where AWS names the app monitor Id the request carried.
AWS: PutRumEvents' only resource identifier in the request is the httpLabel member Id (Smithy: PutRumEvents.Id targets AppMonitorId, @required, @length 36..36, @pattern ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$; uri /appmonitors/{Id}/). Recorded behavior only: the denial message's resource slot is filled with that app monitor Id (the UUID from the request path), not with an ARN.
Emulator: it renders the derived ARN instead — "on resource: arn:<partition>:rum:<region>:<account>:appmonitor/<app-monitor-name>".
Why: the message names an identifier the caller never supplied for this operation (and a name the data-plane caller may not know), so a client correlating the denial back to the Id it sent — the only identifier PutRumEvents takes — cannot match it.
Fix: the PutRumEvents handler's authorization refusal must pass the request's Id as the resource rendered into the message; the ARN remains the resource used for the policy evaluation itself.
Traces: rum.resource-policy-enforced-on-put-rum-events.

## Evidence
Suite: 50/81 passed; all 31 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). 30 of the 31 were marked OWNED by an already-filed issue; rum.resource-policy-enforced-on-put-rum-events is the only surviving failure.

[mismatch] rum.resource-policy-enforced-on-put-rum-events
check_rum_resource_policy_enforced_on_put_rum_events.py, line 54:
  assert code_before[1] == denied
AssertionError:
  expected: 'User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-bare-b0500f2ea0d9/sim2real-behav is not authorized to perform: rum:PutRumEvents on resource: 4d7d1080-9350-40a3-bb93-268e41f11b2b because no identity-based policy allows the rum:PutRumEvents action'
  got:      'User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-bare-b0500f2ea0d9/sim2real-behav is not authorized to perform: rum:PutRumEvents on resource: arn:aws:rum:us-west-2:000000000000:appmonitor/sim2real-behav-appmon-c3983fea0681 because no policy allows the rum:PutRumEvents action'

The assertion one line earlier — that the error code is in the AccessDenied family — passed, so the 403/AccessDeniedException classification is right and only the message diverges. Two independent differences in that one message: (1) the resource slot holds the app monitor ARN where the expectation holds the app monitor Id that was passed as PutRumEvents' Id member (the UUID returned by CreateAppMonitor); (2) the reason clause lacks the literal 'identity-based '. The later legs of the check (PutRumEvents succeeding once the app monitor's resource policy grants rum:PutRumEvents to the bare role, and being denied again after DeleteResourcePolicy) were never reached.

The account id, region, role/session names, app monitor name and the monitor UUID shown above are per-run values of that run and are not part of the requirement; the requirement is the fixed sentence with the principal ARN taken from the signing credentials, the app monitor Id taken from the request, and the action rum:PutRumEvents.

## Steps to Reproduce
1. CreateAppMonitor(Name=<m>, Domain="example.com") and keep the returned Id (a UUID).
2. iam.CreateRole(RoleName=<r>) with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root and NO permissions policies attached.
3. sts.AssumeRole(RoleArn=<role arn>, RoleSessionName="sim2real-behav") and build a rum client from those temporary credentials.
4. With that bare principal, call PutRumEvents(Id=<the monitor Id>, BatchId=<uuid>, AppMonitorDetails={id,name,version}, UserDetails={sessionId,userId}, RumEvents=[{id=<uuid>, timestamp=<now>, type="com.amazon.rum.custom_event", details='{"version":"1.0.0"}', metadata='{"version":"1.0.0","domain":"example.com"}'}]).
5. Observed: 403 AccessDeniedException with Message 'User: <assumed-role arn> is not authorized to perform: rum:PutRumEvents on resource: arn:aws:rum:<region>:<account>:appmonitor/<m> because no policy allows the rum:PutRumEvents action'. Expected: the same status and code with Message 'User: <assumed-role arn> is not authorized to perform: rum:PutRumEvents on resource: <the monitor Id> because no identity-based policy allows the rum:PutRumEvents action'.
6. Then PutResourcePolicy(Name=<m>, PolicyDocument allowing rum:PutRumEvents to that role ARN on the monitor ARN) and repeat step 4 — it must now be accepted; DeleteResourcePolicy(Name=<m>) and repeat step 4 — it must be denied again with the same message as step 5's expectation (the resource policy is evaluated on every call).
7. Replay rum.resource-policy-enforced-on-put-rum-events to confirm.
