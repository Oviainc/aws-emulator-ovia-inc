Issue-Source: trace-sweep-divergence
Issue-Trace: pi-docdb-service-type
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pi-docdb-service-type pi-performance-report-error-paths pi-error-report-not-found
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi|GetResourceMetrics|4xx->2xx|$.Message
Issue-Key: pi|GetResourceMetrics|4xx->2xx|$.__type
Issue-Key: pi|GetResourceMetrics|4xx->2xx|status
Issue-Key: pi|CreatePerformanceAnalysisReport|4xx->2xx|$.Message
Issue-Key: pi|CreatePerformanceAnalysisReport|4xx->2xx|$.__type
Issue-Key: pi|CreatePerformanceAnalysisReport|4xx->2xx|status
Issue-Key: pi|ListPerformanceAnalysisReports|4xx->2xx|$.Message
Issue-Key: pi|ListPerformanceAnalysisReports|4xx->2xx|$.__type
Issue-Key: pi|ListPerformanceAnalysisReports|4xx->2xx|status
Issue-Key: pi|GetPerformanceAnalysisReport|4xx->4xx|$.Message
Issue-Key: pi|GetPerformanceAnalysisReport|4xx->4xx|$.__type
Issue-Key: pi|DeletePerformanceAnalysisReport|4xx->4xx|$.Message
Issue-Key: pi|DeletePerformanceAnalysisReport|4xx->4xx|$.__type
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# pi authorization gate missing: unowned resources answered 200 or InvalidArgumentException

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceMetrics

## Description
Performance Insights (pi) has no resource-authorization gate. Every operation in the service's Smithy model declares NotAuthorizedException as a common error (Smithy: PerformanceInsightsv20180227 common_errors = InternalServiceError, InvalidArgumentException, NotAuthorizedException; NotAuthorizedException is errorKind=client, httpError 400, single member Message, doc "The user is not authorized to perform this request."). AWS uses that single error as the uniform answer whenever the (ServiceType, Identifier) data source — or an AnalysisReportId under it — is not a resource this account may see; it deliberately does NOT disclose whether the resource is absent. The emulator implements no authorization concept at all (the handler for GetResourceMetrics documents this in its own module header: "authorization is not emulated (no IAM principals or policies exist here, so the modelled NotAuthorizedException is never the answer)"), so unowned resources are either SERVED as if owned or refused under the wrong error identity. This bundle carries 5 causes.

Cause 1 of 5 — GetResourceMetrics on an unauthorized DOCDB data source (fixture pi-docdb-service-type, step task#2).
AWS: 400 with $.__type = NotAuthorizedException and a Message (recorded example: 'The specified resource is not authorized for this account'). Smithy: GetResourceMetrics errors include NotAuthorizedException @httpError 400; ServiceType is an enum of RDS | DOCDB, so a DOCDB identifier is a well-formed request that must be authorized, not rejected as malformed.
Emulator: 200 with a GetResourceMetricsOutput envelope; $.__type and $.Message absent.
Why: a client reading metrics for a data source it does not own gets an authoritative-looking success with an empty MetricList instead of an authorization failure. Anything that distinguishes "no telemetry" from "not permitted" — a dashboard, a monitoring agent's retry/alarm logic, an IAM policy test — draws the opposite conclusion, and a cross-account/wrong-ServiceType misconfiguration is silently invisible.
Fix: GetResourceMetrics' handler — add the resource-authorization check (resolve (ServiceType, Identifier) against the service's registered data sources in the service state) ahead of the window arithmetic, and return the modeled NotAuthorizedException when it does not resolve.

Cause 2 of 5 — CreatePerformanceAnalysisReport on an unauthorized data source (fixture pi-performance-report-error-paths, step task#1).
AWS: 400 with $.__type = NotAuthorizedException and a Message (recorded example: 'The specified resource is not authorized for this account'). Smithy: CreatePerformanceAnalysisReport errors include NotAuthorizedException @httpError 400.
Emulator: 200, and the response carries a fabricated AnalysisReportId (output member AnalysisReportId, pattern ^report-[0-9a-f]{17}$); $.__type and $.Message absent.
Why: worse than a read, this is a WRITE that succeeds against a resource the caller may not touch. The client now holds a report id that AWS would never have issued, and subsequent Get/List/Delete calls against it are built on state that cannot exist in the real service.
Fix: CreatePerformanceAnalysisReport's handler — gate on the same data-source authorization check before allocating a report in state.

Cause 3 of 5 — ListPerformanceAnalysisReports on an unauthorized data source (fixture pi-performance-report-error-paths, step task#2).
AWS: 400 with $.__type = NotAuthorizedException and a Message (recorded example: 'The specified resource is not authorized for this account'). Smithy: ListPerformanceAnalysisReports errors include NotAuthorizedException @httpError 400.
Emulator: 200 with a ListPerformanceAnalysisReportsResponse; $.__type and $.Message absent.
Why: an enumeration against a data source the caller is not entitled to is answered as a valid (empty) listing, so a client concludes the instance has no analysis reports rather than that it has no access. Paginating clients treat the absent NextToken as a complete result set.
Fix: ListPerformanceAnalysisReports' handler — same gate, applied before the listing is projected from state.

Cause 4 of 5 — GetPerformanceAnalysisReport for a report id not under the caller's resource (fixture pi-error-report-not-found, step task#1).
AWS: 400 with $.__type = NotAuthorizedException and a Message (recorded example: 'The specified resource is not authorized for this account'). The status (400) already matches; the ERROR CODE does not. Smithy: GetPerformanceAnalysisReport errors are InternalServiceError / InvalidArgumentException / NotAuthorizedException — AnalysisReportId is @required with pattern ^report-[0-9a-f]{17}$, and the requested id satisfies that pattern, so this is an authorization outcome and not an argument-validity one.
Emulator: 400 with $.__type = InvalidArgumentException and a Message rebuilt from internal lookup state (recorded example: 'AnalysisReportId report-00000000000000000 was not found for the resource sim2real-probe-pi-dbinst8-e56fa096.').
Why: the wire error code is what SDK error matching and retry classification branch on, so a client written against AWS never matches this failure as NotAuthorized; it sees a malformed-argument error for a well-formed argument. The substituted message also leaks existence information AWS withholds (it states the report is "not found for" a named resource), which is the disclosure the uniform NotAuthorizedException exists to prevent.
Fix: GetPerformanceAnalysisReport's handler — map an unresolvable report under a resource to the modeled NotAuthorizedException rather than constructing InvalidArgumentException from the lookup miss.

Cause 5 of 5 — DeletePerformanceAnalysisReport for a report id not under the caller's resource (fixture pi-error-report-not-found, step task#3).
AWS: 400 with $.__type = NotAuthorizedException and a Message (recorded example: 'The specified resource is not authorized for this account'). Smithy: DeletePerformanceAnalysisReport errors include NotAuthorizedException @httpError 400; AnalysisReportId is @required, pattern ^report-[0-9a-f]{17}$ and the requested value satisfies it.
Emulator: 400 with $.__type = InvalidArgumentException and the same internal-state-derived Message (recorded example: 'AnalysisReportId report-00000000000000000 was not found for the resource sim2real-probe-pi-dbinst8-e56fa096.').
Why: identical to cause 4 on the delete path — the wrong error code defeats SDK error matching, and cleanup/teardown code that treats NotAuthorized and InvalidArgument differently (one is a permissions problem to surface, the other a bug to fix) takes the wrong branch.
Fix: DeletePerformanceAnalysisReport's handler — same mapping.

No open pi issue covers any of these; the only existing pi issue is its bootstrap issue, which is already resolved. The emulator is up (/__admin/health 200) and state_reset_failures was 0 for this replay, so these are not cross-fixture state bleed.

Traces:
- pi-docdb-service-type#task#2 — GetResourceMetrics, expected 400 NotAuthorizedException, got 200
- pi-performance-report-error-paths#task#1 — CreatePerformanceAnalysisReport, expected 400 NotAuthorizedException, got 200
- pi-performance-report-error-paths#task#2 — ListPerformanceAnalysisReports, expected 400 NotAuthorizedException, got 200
- pi-error-report-not-found#task#1 — GetPerformanceAnalysisReport, expected __type NotAuthorizedException, got InvalidArgumentException
- pi-error-report-not-found#task#3 — DeletePerformanceAnalysisReport, expected __type NotAuthorizedException, got InvalidArgumentException
Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metrics.rs lines 15-19 (module header: authorization deliberately not emulated) and lines 364-369 (invalid_argument helper); services/service-com-amazonaws-pi-performanceinsightsv20180227/src/state.rs (where the resource/report registry a gate would consult lives); services/service-com-amazonaws-pi-performanceinsightsv20180227/smithy-model/com-amazonaws-pi-performanceinsightsv20180227.json (NotAuthorizedException).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, state_reset_failures=0):

1) pi-docdb-service-type#task#2 GetResourceMetrics
   expected_status 400, actual_status 200
   detail: status: expected 400, got 200; $.__type: missing (expected 'NotAuthorizedException', got None); $.Message: missing (expected 'The specified resource is not authorized for this account', got None)

2) pi-performance-report-error-paths#task#1 CreatePerformanceAnalysisReport
   expected_status 400, actual_status 200
   detail: status: expected 400, got 200; $.__type: missing (expected 'NotAuthorizedException', got None); $.Message: missing (expected 'The specified resource is not authorized for this account', got None)

3) pi-performance-report-error-paths#task#2 ListPerformanceAnalysisReports
   expected_status 400, actual_status 200
   detail: status: expected 400, got 200; $.__type: missing (expected 'NotAuthorizedException', got None); $.Message: missing (expected 'The specified resource is not authorized for this account', got None)

4) pi-error-report-not-found#task#1 GetPerformanceAnalysisReport
   expected_status 400, actual_status 400
   detail: $.__type: value_mismatch (expected 'NotAuthorizedException', got 'InvalidArgumentException'); $.Message: value_mismatch (expected 'The specified resource is not authorized for this account', got 'AnalysisReportId report-00000000000000000 was not found for the resource sim2real-probe-pi-dbinst8-e56fa096.')

5) pi-error-report-not-found#task#3 DeletePerformanceAnalysisReport
   expected_status 400, actual_status 400
   detail: $.__type: value_mismatch (expected 'NotAuthorizedException', got 'InvalidArgumentException'); $.Message: value_mismatch (expected 'The specified resource is not authorized for this account', got 'AnalysisReportId report-00000000000000000 was not found for the resource sim2real-probe-pi-dbinst8-e56fa096.')

Requirements asserted: HTTP 400 and wire error code NotAuthorizedException on all five. The quoted Message strings are recorded EXAMPLES of AWS's uniform authorization text; the requirement is the status and the error code, plus the rule that the message must not be rebuilt from internal lookup state (causes 4 and 5 disclose resource existence AWS withholds).

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture pi-docdb-service-type and inspect step task#2 — GetResourceMetrics with ServiceType=DOCDB against a data source the account is not authorized for. Emulator answers 200 with an empty MetricList; AWS answers 400 __type=NotAuthorizedException.
4. Cause 2/3: replay fixture pi-performance-report-error-paths, steps task#1 and task#2 — CreatePerformanceAnalysisReport and ListPerformanceAnalysisReports against the same unauthorized data source. Emulator answers 200 (and mints an AnalysisReportId on task#1); AWS answers 400 __type=NotAuthorizedException.
5. Cause 4/5: replay fixture pi-error-report-not-found, steps task#1 and task#3 — GetPerformanceAnalysisReport and DeletePerformanceAnalysisReport with a pattern-valid AnalysisReportId (report-00000000000000000) that is not under the caller's resource. Emulator answers 400 __type=InvalidArgumentException with a message naming the resource; AWS answers 400 __type=NotAuthorizedException.
6. Confirm a fix by re-replaying pi-docdb-service-type#task#2, pi-performance-report-error-paths#task#1, pi-performance-report-error-paths#task#2, pi-error-report-not-found#task#1 and pi-error-report-not-found#task#3 — all five must pass.
