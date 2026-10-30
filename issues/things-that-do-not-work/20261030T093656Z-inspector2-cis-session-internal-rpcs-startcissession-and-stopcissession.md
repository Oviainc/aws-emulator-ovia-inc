Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.start-cis-session-unauthorized-scan-job-rejected
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.start-cis-session-unauthorized-scan-job-rejected|run#6|AssertionError|assert err.get("Code") == "ValidationException", (
Issue-Commit: 5f287cca7598dba49dbfde37b90c5e553461c260

# inspector2 CIS-session internal RPCs: StartCisSession and StopCisSession answer NotImplemented instead of the ValidationException identity refusal (2 causes)

> _Note: this issue was identified against the code base at commit `5f287cca7598dba49dbfde37b90c5e553461c260`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCisSession

## Description
TWO causes, one shared shape of defect: the two CIS-session internal RPCs of inspector2 are still scaffold stubs and answer the emulator's generic 501 `NotImplemented` refusal instead of the modeled 400 `ValidationException` that AWS serves to every ordinary IAM/STS caller. Neither operation is covered by an auto-filed stub issue, and neither appears in the existing open issue 20261029T202306Z-inspector2-four-behavioral-control-plane-reads-answer-notimplemented.md (that one names GetDelegatedAdminAccount, CreateCodeSecurityIntegration, GetEncryptionKey and GetFindingsReportStatus only), so they are filed here as net-new work items. The requirement in both causes is the modeled HTTP status, error code and message; the scanJobId and sessionToken in the request are request-derived values that do NOT appear in the message at all, so there is no per-run value to template.

Cause 1 of 2: StartCisSession called by a caller that is not the Inspector2-managed SSM CIS-scan plugin.
AWS: Smithy: StartCisSession, PUT /cissession/start, input {scanJobId (required, UUID, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$), message (required, StartCisSessionMessage)}, output StartCisSessionResponse (empty, 200); modeled errors AccessDeniedException (403), ConflictException (409), InternalServerException (500), ThrottlingException (429), ValidationException (400). The operation's own model documentation states it "is used by the Amazon Inspector SSM plugin to communicate with the Amazon Inspector service" for a "scan ID supplied by the service" — no ordinary caller can ever hold a service-supplied scanJobId. Recorded behavior: for ANY scanJobId presented by such a caller, valid-shaped or not, AWS answers HTTP 400 ValidationException with message "Identity user principal does not contain valid resourceId". That whole sentence is FIXED text: it names no scanJobId, no session token, no account and no timestamp, because the refusal tracks the CALLER's principal (which carries no scan resourceId) and not the job id. The refusal is therefore unconditional for non-plugin callers, and must not depend on whether a CIS scan job of that id exists.
Emulator: answers error code 'NotImplemented' with message "operation StartCisSession is scaffolded but not implemented in this emulator" (HTTP 501). That code is in no inspector2 Smithy model, so no SDK error-shape factory maps it and botocore surfaces a bare ClientError.
Why: a client (and the real SSM plugin's own retry/abort logic) cannot distinguish "this principal may not drive CIS sessions" from "the service is broken". A 400 ValidationException is a terminal, non-retryable client fault, whereas 501 is neither modeled nor classified, so SDK retry policies and any code branching on ValidationException take the unexpected-error path; worse, a caller probing the CIS-session surface gets no signal that the identity gate exists at all.
Fix: implement the StartCisSession handler and its caller-principal/resourceId gate, which must precede any scanJobId lookup or session write, in the inspector2 crate's CIS-session state (services/service-com-amazonaws-inspector2-inspector2/src/ops/start_cis_session.rs — the file still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("StartCisSession"))` at the end of `handle`).
Traces: inspector2.start-cis-session-unauthorized-scan-job-rejected.

Cause 2 of 2: StopCisSession on a session that was never started, called by the same non-plugin caller.
AWS: Smithy: StopCisSession, PUT /cissession/stop, input {scanJobId (required, UUID, same @pattern), sessionToken (required, UUID, same @pattern), message (required, StopCisSessionMessage)}, output StopCisSessionResponse (empty, 200); modeled errors AccessDeniedException (403), ConflictException (409), InternalServerException (500), ThrottlingException (429), ValidationException (400). Recorded behavior: the identical identity gate applies, so AWS answers HTTP 400 ValidationException with the same fixed message "Identity user principal does not contain valid resourceId" — again with no request- or state-derived variable part. Because there is no customer-facing read of CIS session state, this call is the independent confirmation that StartCisSession produced no effect: the never-started session must remain unusable and fail identically, rather than succeeding or failing some other way.
Emulator: answers error code 'NotImplemented' with message "operation StopCisSession is scaffolded but not implemented in this emulator" (HTTP 501).
Why: because the stop path answers the same non-modeled 501, a client cannot confirm that no session was created and cannot clean up; the "effect absent" property of a refused StartCisSession is unobservable, so an emulator that later implements StartCisSession permissively would be indistinguishable from a correct one.
Fix: implement the StopCisSession handler with the same caller-principal/resourceId gate applied before any session-state lookup, in services/service-com-amazonaws-inspector2-inspector2/src/ops/stop_cis_session.rs (also still carrying the `SCAFFOLDED, NOT IMPLEMENTED` marker). Both handlers should share one identity-gate helper so the two refusals cannot drift apart.
Traces: inspector2.start-cis-session-unauthorized-scan-job-rejected.

## Evidence
Check inspector2.start-cis-session-unauthorized-scan-job-rejected (verdict: mismatch).

Positive control in the same check PASSED: inspector2.list_usage_totals() returned a response containing `totals`, so the caller's credentials and the service's wiring are healthy for inspector2 — the refusals below are not "everything is denied".

Cause 1 — assertion `assert err.get("Code") == "ValidationException"` failed:
  AssertionError: expected StartCisSession to reject a non-SSM-plugin caller with ValidationException, got 'NotImplemented'
  Expected: HTTP 400, Error.Code "ValidationException", Error.Message "Identity user principal does not contain valid resourceId" (entire message is fixed text).
  Actual:   HTTP 501, Error.Code "NotImplemented", Error.Message "operation StartCisSession is scaffolded but not implemented in this emulator".

Cause 2 — the check's second assertion pair (`assert stop_err.get("Code") == "ValidationException"` / `assert stop_err.get("Message") == "Identity user principal does not contain valid resourceId"`) is unreachable only because cause 1 aborts the check first; the StopCisSession handler is the same scaffold stub, so it answers HTTP 501 Error.Code "NotImplemented" with message "operation StopCisSession is scaffolded but not implemented in this emulator" instead of the identical 400 ValidationException refusal.

Source corroboration: services/service-com-amazonaws-inspector2-inspector2/src/ops/start_cis_session.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED` and line 164 returns `Err(crate::scaffolded::Scaffolded::not_implemented("StartCisSession"))`; services/service-com-amazonaws-inspector2-inspector2/src/ops/stop_cis_session.rs line 4 carries the same marker. The inspector2 model already declares ValidationException with httpError 400 for both operations, so only the handler and its identity gate are missing.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1) for service inspector2.
2. Positive control: call ListUsageTotals and confirm a 200 response containing `totals` (the caller is healthy for inspector2).
3. Cause 1: call StartCisSession (PUT /cissession/start) with scanJobId set to any freshly generated UUID matching ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$ and message={"sessionToken": <any UUID>}, using ordinary IAM/STS credentials (i.e. not the Inspector2-managed SSM plugin principal). Expect HTTP 400 ValidationException with message "Identity user principal does not contain valid resourceId"; observe HTTP 501 with Error.Code "NotImplemented".
4. Cause 2: call StopCisSession (PUT /cissession/stop) with the SAME scanJobId, sessionToken=<any UUID> and message={"status": "SUCCESS", "progress": {"totalChecks": 0}}. Expect the identical HTTP 400 ValidationException with message "Identity user principal does not contain valid resourceId"; observe HTTP 501 with Error.Code "NotImplemented".
5. Re-replay check inspector2.start-cis-session-unauthorized-scan-job-rejected to confirm both causes are closed — the check asserts the code AND the exact message for both operations, so fixing only StartCisSession will leave it failing.
