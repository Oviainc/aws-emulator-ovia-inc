Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-audit-findings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-audit-findings application-signals-discovery-and-service-topology
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|ListAuditFindings|2xx->4xx|$.AuditFindings
Issue-Key: application-signals|ListAuditFindings|2xx->4xx|$.EndTime
Issue-Key: application-signals|ListAuditFindings|2xx->4xx|$.StartTime
Issue-Key: application-signals|ListAuditFindings|2xx->4xx|status
Issue-Key: application-signals|ListAuditFindings|4xx->4xx|$.message
Issue-Key: application-signals|GetService|2xx->4xx|$.EndTime
Issue-Key: application-signals|GetService|2xx->4xx|$.Service
Issue-Key: application-signals|GetService|2xx->4xx|$.StartTime
Issue-Key: application-signals|GetService|2xx->4xx|status
Issue-Key: application-signals|ListServiceOperations|2xx->4xx|$.EndTime
Issue-Key: application-signals|ListServiceOperations|2xx->4xx|$.ServiceOperations
Issue-Key: application-signals|ListServiceOperations|2xx->4xx|$.StartTime
Issue-Key: application-signals|ListServiceOperations|2xx->4xx|status
Issue-Key: application-signals|ListServiceDependencies|2xx->4xx|$.EndTime
Issue-Key: application-signals|ListServiceDependencies|2xx->4xx|$.ServiceDependencies
Issue-Key: application-signals|ListServiceDependencies|2xx->4xx|$.StartTime
Issue-Key: application-signals|ListServiceDependencies|2xx->4xx|status
Issue-Key: application-signals|ListServiceDependents|2xx->4xx|$.EndTime
Issue-Key: application-signals|ListServiceDependents|2xx->4xx|$.ServiceDependents
Issue-Key: application-signals|ListServiceDependents|2xx->4xx|$.StartTime
Issue-Key: application-signals|ListServiceDependents|2xx->4xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# application-signals rejects epoch-second StartTime/EndTime query timestamps, so every time-ranged read answers 400 instead of 200

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAuditFindings

## Description
Six causes, all rooted in the service's handling of the required `StartTime`/`EndTime` time-range inputs. In the Smithy model every one of these operations binds both as `@httpQuery` timestamps (ListAuditFindings.StartTime/EndTime -> target Timestamp, httpQuery, required; same on GetService, ListServiceOperations, ListServiceDependencies, ListServiceDependents), and the only modeled client error is ValidationException (400). The emulator's query-string timestamp deserializer refuses the numeric epoch-second form the recorded clients send, so each of these reads fails at input binding and never reaches its handler.

Cause 1 of 6: ListAuditFindings (POST /auditFindings) on a valid time range.
AWS: Smithy ListAuditFindings — StartTime/EndTime are required @httpQuery Timestamp members; output is StartTime, EndTime and the required AuditFindings list; success is HTTP 200 with the modeled body. The recorded response is 200 with AuditFindings = [] and both timestamps echoed.
Emulator: answers HTTP 400 with no AuditFindings, StartTime or EndTime member at all. The sibling step shows the reason in the message text: `invalid timestamp \`1788660000\`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times` — the query-value parser only accepts an RFC-3339 string and rejects the epoch-second number.
Why: a client paging audit findings over a time window gets a 400 ValidationException for a perfectly valid request. Nothing on this operation is reachable, and because every member of the success body is missing there is no partial result to work with.
Fix: the query-parameter timestamp deserialization for this service's input bindings — an @httpQuery timestamp must accept the epoch-seconds form in addition to RFC-3339 date-time, before the operation handler runs.
Traces: application-signals-audit-findings#task#1.

Cause 2 of 6: ListAuditFindings with StartTime after EndTime — the time-range ordering validation never fires.
AWS: the modeled rejection is ValidationException (400). The recorded response is 400 whose message is built from the request's two timestamps and states the rule: `Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time` (recorded text is an example; the requirement is that an inverted range is refused by an ordering check that names the offending times).
Emulator: returns 400, but the body is the same timestamp-parse failure as Cause 1 (`invalid timestamp \`1788660000\`: ... Smithy does not support timezone offsets in RFC-3339 date times`). The status coincides; the ordering check does not exist, because binding fails first.
Why: the status is right by accident. Once the parser is fixed this request would be accepted as valid rather than refused, so the inverted-range guard has to be added as well — otherwise a client with swapped bounds gets data instead of a ValidationException.
Fix: ListAuditFindings' input validation — add the StartTime < EndTime guard, raising ValidationException once the timestamps parse.
Traces: application-signals-audit-findings#task#2.

Cause 3 of 6: GetService (POST /service).
AWS: Smithy GetService — required @httpQuery StartTime/EndTime plus required KeyAttributes; output has required Service, StartTime and EndTime; success 200. The recorded response is 200 carrying Service plus both echoed timestamps.
Emulator: HTTP 400 with Service, StartTime and EndTime all absent — the same query-timestamp refusal.
Why: service discovery is unusable; a client cannot read any service entity.
Fix: same query-timestamp deserialization path; GetService's handler is never entered.
Traces: application-signals-discovery-and-service-topology#task#3.

Cause 4 of 6: ListServiceOperations (POST /service-operations).
AWS: Smithy ListServiceOperations — required @httpQuery StartTime/EndTime; output has required StartTime, EndTime and ServiceOperations; success 200. The recording shows 200 with ServiceOperations = [].
Emulator: HTTP 400, no ServiceOperations/StartTime/EndTime.
Why: the paginated operations listing cannot be called at all.
Fix: same deserialization path.
Traces: application-signals-discovery-and-service-topology#task#4.

Cause 5 of 6: ListServiceDependencies.
AWS: required @httpQuery StartTime/EndTime; success 200 with StartTime, EndTime and ServiceDependencies. Recording: 200 with ServiceDependencies = [].
Emulator: HTTP 400 with all three members absent.
Why: topology dependency reads are unavailable.
Fix: same deserialization path.
Traces: application-signals-discovery-and-service-topology#task#5.

Cause 6 of 6: ListServiceDependents.
AWS: required @httpQuery StartTime/EndTime; success 200 with StartTime, EndTime and ServiceDependents. Recording: 200 with ServiceDependents = [].
Emulator: HTTP 400 with all three members absent.
Why: topology dependent reads are unavailable.
Fix: same deserialization path.
Traces: application-signals-discovery-and-service-topology#task#6.

Note: a seventh step in the discovery fixture (task#8, a GetService validation path) is blocked by an unimplemented GetService stub and is tracked by its own auto-filed stub issue, not here.

## Evidence
Cause 1 — application-signals-audit-findings#task#1 ListAuditFindings: expected status 200, got 400; $.AuditFindings missing (expected []), $.StartTime missing, $.EndTime missing.
Cause 2 — application-signals-audit-findings#task#2 ListAuditFindings: status 400 both sides, but $.message expected 'Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time', got 'invalid timestamp `1788660000`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times'.
Cause 3 — application-signals-discovery-and-service-topology#task#3 GetService: expected 200, got 400; $.Service, $.StartTime (1788660000.0), $.EndTime (1788674401.0) all missing.
Cause 4 — ...#task#4 ListServiceOperations: expected 200, got 400; $.ServiceOperations (expected []), $.StartTime, $.EndTime missing.
Cause 5 — ...#task#5 ListServiceDependencies: expected 200, got 400; $.ServiceDependencies (expected []), $.StartTime, $.EndTime missing.
Cause 6 — ...#task#6 ListServiceDependents: expected 200, got 400; $.ServiceDependents (expected []), $.StartTime, $.EndTime missing.
Smithy citations: ListAuditFindings/GetService/ListServiceOperations/ListServiceDependencies/ListServiceDependents each declare StartTime and EndTime as required @httpQuery Timestamp members with ValidationException (400) as the only modeled client error.

## Steps to Reproduce
1. Replay fixture application-signals-audit-findings, step task#1: call ListAuditFindings with AuditTargets and a StartTime/EndTime query pair expressed as epoch seconds (e.g. StartTime=1788660000, EndTime=1788674401). Emulator answers 400 instead of 200 with an empty AuditFindings list.
2. Replay application-signals-audit-findings step task#2 with StartTime after EndTime: the 400 body carries the timestamp-parse message instead of the start-must-be-before-end validation message.
3. Replay application-signals-discovery-and-service-topology steps task#3 (GetService), task#4 (ListServiceOperations), task#5 (ListServiceDependencies) and task#6 (ListServiceDependents) with the same epoch-second time range: each answers 400 with an empty body instead of 200.
