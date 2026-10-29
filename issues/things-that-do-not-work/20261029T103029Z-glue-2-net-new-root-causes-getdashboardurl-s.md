Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.dashboard-url-returns-valid-endpoint glue.session-idle-timeout-transitions-to-timed-out
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.dashboard-url-returns-valid-endpoint|run#14|AssertionError|assert message == "Entity not found", f"unexpected not-found message {message!r}"
Issue-Key: glue.session-idle-timeout-transitions-to-timed-out|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 56752714ba65963d122a47feae790978b4aace00

# Glue: 2 net-new root causes — GetDashboardUrl's unknown-session refusal message is emulator-authored, and an interactive Session's IdleTimeout is never enforced (never leaves READY for TIMEOUT)

> _Note: this issue was identified against the code base at commit `56752714ba65963d122a47feae790978b4aace00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboardUrl

## Description
Two independent Glue interactive-Session root causes, both grounded in failing behavioral checks of the glue service. 2 causes total; both must be closed before submitting.

Related open issues, named for the fixer, neither of which carries these checks' replay keys: 20261028T122732Z-glue-4-net-new-root-causes-four-refusal.md (its Cause 3 describes the SAME GetDashboardUrl message defect but is grounded only in the check glue.get-dashboard-url-returns-valid-resource-url, so re-replaying that issue's trace list will NOT exercise glue.dashboard-url-returns-valid-endpoint — the cause is restated here so this check is covered); no open issue covers session idle-timeout expiry at all.

Cause 1 of 2: GetDashboardUrl on a ResourceId that names no session returns the wrong error MESSAGE.
AWS: Smithy: GetDashboardUrl takes required ResourceId (NameString, @length 1..255) and required ResourceType (GlueResourceType enum), and models EntityNotFoundException (client, HTTP 400) among its errors. Recorded behavior: for ResourceType=SESSION with a ResourceId that names no session — both an id that never existed and the id of a session that was just deleted — real Glue refuses with HTTP 400, Error.Code `EntityNotFoundException` and Error.Message exactly `Entity not found`. That message is entirely FIXED text: it has no variable parts at all — no resource id, no resource type, no trailing period. It is deliberately distinct from GetSession's not-found message, which for the same missing session IS templated: fixed text `Session ` + the Id taken verbatim from the request + fixed text ` not found.` (the check asserts that GetSession form separately, and the emulator gets GetSession right).
Emulator: HTTP status and Error.Code are correct (400 / EntityNotFoundException), but Error.Message is GetSession's templated sentence: `Session <ResourceId> not found.` — i.e. fixed text `Session `/` not found.` with the request's ResourceId interpolated. Replay: `AssertionError: unexpected not-found message 'Session sim2real-glue-dash-missing-<suffix> not found.'` (the suffix is a per-run generated id and is not itself the defect; the defect is that any id appears at all, and that the fixed wording differs).
Why: `Entity not found` is the only thing that distinguishes this refusal on the wire, so a client, SDK helper or test that branches on or asserts the documented GetDashboardUrl message never matches, and a caller that parses the message sees a sentence shape AWS never emits for this operation. It also makes the message vary per request where AWS keeps it invariant.
Fix: the GetDashboardUrl handler's session-lookup miss must raise EntityNotFoundException with the bare fixed text instead of reusing/formatting the GetSession not-found template; the GetSession handler's own message must stay as it is. Belongs in the GetDashboardUrl operation handler of the glue service crate (services/service-com-amazonaws-glue-awsglue/src/ops/get_dashboard_url.rs), alongside the already-correct ResourceType=JOB OperationNotSupportedException path.
Traces: glue.dashboard-url-returns-valid-endpoint (assertion at check line 55; the same check re-asserts the identical message for GetDashboardUrl against the session AFTER DeleteSession, so the fix must cover the deleted-session path too).

Cause 2 of 2: an idle interactive Session never honours its IdleTimeout — it does not transition READY -> TIMEOUT.
AWS: Smithy: CreateSession accepts IdleTimeout (target Timeout, integer, @range min 1 — minutes); Session$IdleTimeout is documented as "The number of minutes when idle before the session times out"; Session$Status targets the SessionStatus enum whose values are PROVISIONING, READY, FAILED, TIMEOUT, STOPPING, STOPPED, and Session also carries CompletedOn ("The date and time that this session is completed") and ExecutionTime. So TIMEOUT is a modeled terminal state reachable without any client call: after a session becomes READY and no statement is run for IdleTimeout minutes, real Glue expires it server-side and GetSession thereafter reports Status TIMEOUT (and stops billing it). The transition must also not fire EARLY: it must not be observable before the declared idle window has substantially elapsed (the check allows no sooner than half of IdleTimeout).
Emulator: GetSession keeps answering with a non-terminal Status for a session created with IdleTimeout=1 and left completely idle; the check never observes any of TIMEOUT/FAILED/STOPPED while polling GetSession every 10s and is killed when it exhausts its wall-clock budget (`Error: Check timed out after 60.0s`). Hypothesis: there is no idle-expiry reaper for sessions at all — IdleTimeout is stored and echoed but never acted on, so TIMEOUT is a SessionStatus value no code path can produce and the session stays READY indefinitely until DeleteSession.
Why: a client that provisions an interactive session and relies on the service to reclaim it (the normal cost-control contract for Glue sessions) sees a session that stays READY forever; any workflow that waits for, or branches on, the TIMEOUT terminal state hangs, and Session.CompletedOn/ExecutionTime are never populated for an expired session. Nothing a caller can do reproduces the server-side expiry, so this is unobservable fidelity loss rather than a slow path.
Fix: session state/lifecycle for the glue service crate — the session store needs a time-driven expiry that, once a session is READY and has gone IdleTimeout minutes without a statement being submitted (RunStatement activity resets the idle clock), moves Status to TIMEOUT and stamps CompletedOn/ExecutionTime, so GetSession/ListSessions report the terminal state. It belongs in the session state component rather than in any single handler, since GetSession must observe it without a client call. Note the two-sided requirement: the transition must not be made immediate on READY, or the same check fails the other way.
Traces: glue.session-idle-timeout-transitions-to-timed-out (CreateSession with IdleTimeout=1, WorkerType G.1X, NumberOfWorkers 2; poll GetSession for a terminal status, then assert the status is TIMEOUT and that it was not observed sooner than 30s after READY).

## Evidence
Cause 1 — glue.dashboard-url-returns-valid-endpoint:
  call:     GetDashboardUrl(ResourceId=<an id naming no session>, ResourceType='SESSION')
  expected: HTTP 400 / EntityNotFoundException / Message exactly 'Entity not found'
            (message is 100% fixed text — no request- or state-derived parts)
  actual:   HTTP 400 / EntityNotFoundException / Message 'Session <ResourceId> not found.'
            (fixed text 'Session ' + the request's ResourceId + ' not found.')
  replay:   AssertionError: unexpected not-found message 'Session sim2real-glue-dash-missing-c690ac4759f4 not found.'
            (the 'c690ac4759f4' suffix is a per-run generated name, not part of the requirement)
  contrast: the SAME check asserts GetSession(Id=<deleted session>) must answer
            EntityNotFoundException / 'Session <Id> not found.' — the templated form IS correct
            there, which is why the two messages must not share one code path.
  scope:    the check asserts the 'Entity not found' message twice — for an id that never
            existed and for the id of a session deleted by DeleteSession.

Cause 2 — glue.session-idle-timeout-transitions-to-timed-out:
  setup:    CreateSession(Id=<new>, Role=<glue service role>, Command={glueetl,3},
            GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, IdleTimeout=1) and then
            NO statement is ever run against the session.
  expected: GetSession(Id).Session.Status leaves PROVISIONING for READY, and then — with the
            session still idle — becomes TIMEOUT (a modeled SessionStatus value), no sooner
            than half the declared IdleTimeout window after READY.
  actual:   GetSession keeps reporting a non-terminal status; none of TIMEOUT / FAILED /
            STOPPED is ever observed, and the check is killed while still polling.
  replay:   Error: Check timed out after 60.0s
  note:     the observation is the exhausted budget while polling GetSession; the stated
            cause (no idle-expiry reaper, so TIMEOUT is unreachable) is labelled Hypothesis
            in the description.

## Steps to Reproduce
Cause 1:
1. Create a Glue interactive session: CreateSession(Id=S, Role=<arn of a role glue.amazonaws.com can assume>, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='4.0', DefaultArguments={'--enable-spark-live-ui':'true'}); poll GetSession(Id=S) until Status=READY.
2. GetDashboardUrl(ResourceId=S, ResourceType='SESSION') -> expect an https:// Url (this part already passes).
3. GetDashboardUrl(ResourceId=<a fresh id that names no session>, ResourceType='SESSION'). Read Error.Code and Error.Message: Code must be EntityNotFoundException and Message must be exactly the string 'Entity not found' — no id, no resource type, no trailing period.
4. DeleteSession(Id=S). Then GetSession(Id=S) must be EntityNotFoundException with Message 'Session ' + S + ' not found.' (templated — unchanged), while GetDashboardUrl(ResourceId=S, ResourceType='SESSION') must be EntityNotFoundException with Message exactly 'Entity not found'. The two messages must differ.

Cause 2:
1. Create an IAM role trusting glue.amazonaws.com with AWSGlueServiceRole attached.
2. CreateSession(Id=S, Role=<that role arn>, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, IdleTimeout=1).
3. Poll GetSession(Id=S).Session.Status until it is no longer PROVISIONING; it must be READY. Record the time.
4. Run NO statement. Keep polling GetSession(Id=S).Session.Status every 10s.
5. The status must become TIMEOUT, and must not be observed as TIMEOUT sooner than 30s (half of IdleTimeout=1 minute) after READY. Today no terminal status (TIMEOUT/FAILED/STOPPED) is ever reached and the poll runs until the caller gives up.
