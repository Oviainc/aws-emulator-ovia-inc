Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.create-dashboard-double-space-name-rejected bcm-dashboards.create-dashboard-widget-count-exceeds-max-rejected bcm-dashboards.delete-dashboard-removes-it bcm-dashboards.delete-nonexistent-dashboard-idempotent-noop bcm-dashboards.create-scheduled-report-idempotent-token-concurrent
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.create-dashboard-double-space-name-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: bcm-dashboards.create-dashboard-widget-count-exceeds-max-rejected|run#8|AssertionError|assert message == expected_message, message
Issue-Key: bcm-dashboards.delete-dashboard-removes-it|run#7|AssertionError|assert message == f"Resource {arn} does not exist.", message
Issue-Key: bcm-dashboards.delete-nonexistent-dashboard-idempotent-noop|run#7|AssertionError|assert message == f"Resource {arn} does not exist.", message
Issue-Key: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent|run#11|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {successes}"
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# bcm-dashboards: DashboardName pattern unenforced, constraint/not-found error messages diverge, and CreateScheduledReport clientToken has no single-winner semantics (4 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDashboard

## Description
Bundled behavioral divergences in the bcm-dashboards emulator. FOUR distinct root causes, each with its own failing checks; all four must be closed before this issue is submitted.

=== Cause 1 of 4: CreateDashboard accepts a `name` that violates the modeled DashboardName pattern (overly permissive) ===
AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) — CreateDashboard.name targets DashboardName, @required, @length min 2 max 50, @pattern `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$`. The negative lookahead forbids two consecutive spaces. CreateDashboard models ValidationException (HTTP 400); a name containing a double space is refused with code `ValidationException` at HTTP 400 and no dashboard is created. The recorded message is, as an example, "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]".
Emulator: the call SUCCEEDS with HTTP 200 and returns an `arn` (e.g. arn:aws:bcm-dashboards::000000000000:dashboard/000...01), persisting a dashboard whose name real AWS would never accept. The boundary validator clearly runs some constraints for this service (it does enforce the widgets @length — see cause 2), but the @pattern on DashboardName is not applied.
Why: a client that relies on AWS rejecting malformed names gets a resource created in the emulator that cannot exist in production. Name-validation error paths cannot be tested at all, and later reads return a name outside the modeled pattern, which a strict SDK/output validator can reject.
Fix: the request-validation layer for this service's constrained string shapes — the @pattern of DashboardName must be enforced on CreateDashboard (and, since the same shape is @required there, UpdateDashboard.name) before the handler runs, answering the modeled ValidationException at HTTP 400 and leaving state untouched.
Traces: bcm-dashboards.create-dashboard-double-space-name-rejected

=== Cause 2 of 4: CreateDashboard's list-length ValidationException message does not echo the offending value ===
AWS: Smithy — CreateDashboard.widgets targets WidgetList with @length min 0 max 20; error ValidationException (HTTP 400). The constraint-violation message is built from the request in the standard form "1 validation error detected: Value '<the offending value>' at '<member>' failed to satisfy constraint: Member must have length less than or equal to 20" — the value position carries the rejected value itself, not a summary of it. Recorded example: the value position holds the serialized widget list ("[Widget(id=null, title=Widget 0, ...), ...]").
Emulator: status 400 and code `ValidationException` are CORRECT, but the message is "1 validation error detected: Value '21 items' at 'widgets' failed to satisfy constraint: Member must have length less than or equal to 20" — the value position is replaced with the placeholder "21 items".
Why: the constraint-message rule is request-derived and deterministic, so clients and test suites that parse the value position out of a ValidationException (to report which input was rejected) read a placeholder instead of the input. Message-only divergence, hence low severity on its own.
Fix: the constraint-violation message formatter in this service's validation layer — render the rejected member's value in the value position for @length violations on aggregate shapes rather than an item count.
Traces: bcm-dashboards.create-dashboard-widget-count-exceeds-max-rejected

=== Cause 3 of 4: GetDashboard's ResourceNotFoundException message does not follow the service's request-derived template ===
AWS: Smithy — GetDashboard models ResourceNotFoundException (HTTP 404) for an ARN no dashboard is stored under. The message is built from the request ARN with the template "Resource <arn> does not exist." (UpdateDashboard uses its own request-derived template, "Dashboard not found: <arn>", which is why these are per-operation templates and not incidental text).
Emulator: status 404 and code `ResourceNotFoundException` are CORRECT, but the message is "No dashboard found for arn <arn>" — see services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/get_dashboard.rs:175, which formats that string.
Why: both checks that delete a dashboard and then confirm it is gone assert the request-derived not-found template and fail before they can verify the rest of the contract (notably that a repeat DeleteDashboard on a well-formed, already-deleted ARN is a 200 no-op — DeleteDashboard models no ResourceNotFoundException at all, so that assertion is still unverified and should be re-checked once the message is fixed).
Fix: the not-found error construction in the GetDashboard handler (and the matching template in the UpdateDashboard handler).
Traces: bcm-dashboards.delete-dashboard-removes-it, bcm-dashboards.delete-nonexistent-dashboard-idempotent-noop

=== Cause 4 of 4: CreateScheduledReport has no clientToken-keyed single-winner behavior; every concurrent call fails ===
AWS: Smithy — CreateScheduledReport.clientToken targets ClientToken, @idempotencyToken, header X-Amzn-Client-Token, @length 1..128, @pattern `^[\u0021-\u007E]+$`; the operation models ConflictException (HTTP 409). Recorded behavior: N concurrent CreateScheduledReport calls sharing one clientToken and identical parameters produce exactly ONE success (one scheduled report ever created under that token, listed exactly once by ListScheduledReports); the racing losers are refused with code `ConflictException` at HTTP 409 (recorded message indicates a create is already in progress) rather than each getting a mirrored success.
Emulator: all 5 concurrent calls fail — zero successes (`expected exactly one winner, got []`). CreateScheduledReport is an unimplemented scaffold stub (services/.../src/ops/create_scheduled_report.rs), which is filed separately as its own already-filed stub issue; that issue only requires the operation to exist. This cause records the additional fidelity requirement that implementation must satisfy, and must not be closed merely by making the operation answer: the clientToken must key an at-most-once create, with concurrent duplicate-token callers receiving ConflictException (409) and exactly one report of that name appearing in ListScheduledReports.
Why: idempotency-token clients retry on timeout; an emulator that either fails every call or mints a report per call makes retry-safe creation impossible to develop against and can silently duplicate scheduled reports.
Fix: the CreateScheduledReport handler plus the service state that must record in-flight/completed client tokens (token -> minted ARN) so a duplicate token resolves to one winner and refuses the racers with the modeled ConflictException.
Traces: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent

## Evidence
Suite: simparity-behavioral-aws-v1 — bcm-dashboards 5/26 passed (19%), 21 failed.

Cause 1 — create-dashboard-double-space-name-rejected [mismatch]:
  Expected: ClientError code ValidationException, HTTP 400, no dashboard created.
  Actual: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'arn': 'arn:aws:bcm-dashboards::000000000000:dashboard/00000000000000000000000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 2 — create-dashboard-widget-count-exceeds-max-rejected [mismatch]:
  Expected message (rule: request-derived constraint message echoing the rejected value): "1 validation error detected: Value '[Widget(id=null, title=Widget 0, ...), ...]' at 'widgets' failed to satisfy constraint: Member must have length less than or equal to 20"
  Actual message: "1 validation error detected: Value '21 items' at 'widgets' failed to satisfy constraint: Member must have length less than or equal to 20"
  (code ValidationException / HTTP 400 matched)

Cause 3 — delete-dashboard-removes-it and delete-nonexistent-dashboard-idempotent-noop [mismatch]:
  Expected message: "Resource arn:aws:bcm-dashboards::000000000000:dashboard/<id> does not exist."
  Actual message:   "No dashboard found for arn arn:aws:bcm-dashboards::000000000000:dashboard/00000000000000000000000000000001"
  (code ResourceNotFoundException / HTTP 404 matched). Source of the actual string: src/ops/get_dashboard.rs:175.

Cause 4 — create-scheduled-report-idempotent-token-concurrent [mismatch]:
  Expected: 1 success + 4 ConflictException (HTTP 409) across 5 concurrent same-clientToken calls; exactly one report of that name listed.
  Actual: AssertionError: expected exactly one winner, got []  (all 5 calls raised a ClientError).

## Steps to Reproduce
Boot the emulator and point a botocore client at the bcm-dashboards endpoint.

Cause 1: create_dashboard(name="sim2real-dash-xyz  bad" (two consecutive spaces), widgets=[one cost-and-usage widget]). Expect ValidationException/400; observe HTTP 200 and a minted ARN. list_dashboards shows the new dashboard.

Cause 2: create_dashboard(name=<valid>, widgets=[21 cost-and-usage widgets]). Expect ValidationException/400 whose message echoes the rejected widgets value; observe the value position rendered as '21 items'.

Cause 3: create_dashboard(...) -> delete_dashboard(arn=arn) -> get_dashboard(arn=arn). Expect ResourceNotFoundException with message "Resource <arn> does not exist."; observe "No dashboard found for arn <arn>". Then re-run delete_dashboard(arn=arn) and confirm it is a 200 no-op (DeleteDashboard models no ResourceNotFoundException).

Cause 4: create a dashboard and an execution role, then issue 5 parallel create_scheduled_report calls with identical scheduledReport parameters and one shared clientToken. Expect exactly 1 success and 4 ConflictException/409, and exactly one report of that name from list_scheduled_reports; observe 0 successes.
