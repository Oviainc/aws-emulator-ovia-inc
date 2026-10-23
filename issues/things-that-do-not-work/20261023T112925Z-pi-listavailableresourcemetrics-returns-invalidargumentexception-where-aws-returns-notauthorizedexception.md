Issue-Source: trace-sweep-divergence
Issue-Trace: pi-docdb-service-type
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pi-docdb-service-type
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi|ListAvailableResourceMetrics|4xx->4xx|$.Message
Issue-Key: pi|ListAvailableResourceMetrics|4xx->4xx|$.__type
Issue-Commit: 9f554b772a9de03d86bf95d5b5a196b9ea63eb3c

# pi ListAvailableResourceMetrics returns InvalidArgumentException where AWS returns NotAuthorizedException

> _Note: this issue was identified against the code base at commit `9f554b772a9de03d86bf95d5b5a196b9ea63eb3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAvailableResourceMetrics

## Description
Cause 1 of 1: ListAvailableResourceMetrics picks the wrong modeled error class for a DOCDB request whose resource the caller may not read, because the emulator runs metric-type/ServiceType argument validation ahead of (and instead of) the resource authorization check.

AWS: Smithy model services/service-com-amazonaws-pi-performanceinsightsv20180227/smithy-model/com-amazonaws-pi-performanceinsightsv20180227.json — ListAvailableResourceMetrics takes required ServiceType (enum RDS|DOCDB), Identifier (IdentifierString, @length 0..256, pattern ^[a-zA-Z0-9-]+$) and MetricTypes (MetricTypeList of SanitizedString), and models exactly three errors: InternalServiceError (server, 500), InvalidArgumentException (client, 400, "One of the arguments provided is invalid for this request.") and NotAuthorizedException (client, 400, "The user is not authorized to perform this request."). The model does not state the evaluation order of authorization versus argument validation, so for that ordering this is recorded behavior only: the recording for a DOCDB identifier with metric type "db" shows AWS answering HTTP 400 with __type NotAuthorizedException and Message "The specified resource is not authorized for this account" (that message text is an example — no documented or request-derived rule produces it — but the 400 status and the NotAuthorizedException code are the requirement). Hypothesis: real PI resolves and authorizes the Identifier against the account before it ever inspects MetricTypes, so an unauthorized/unresolvable DOCDB resource never reaches metric-type validation.

Emulator: answers HTTP 400 (status matches) but with __type InvalidArgumentException and Message 'The metric type "db" is not published for a DOCDB data source; the metric types published for it are os.' — i.e. it skipped the authorization step entirely and reported an argument-validation failure instead.

Why: a client relying on AWS's behavior branches on the error code, not the status. Both modeled errors are 400, so the status alone cannot disambiguate them: SDK error-matching and retry/escalation logic that keys on NotAuthorizedException (surface a permissions/ownership problem to the operator, stop retrying, do not rewrite the request) instead sees InvalidArgumentException and concludes the caller sent bad MetricTypes. Callers will loop rewriting the metric list for a resource they are simply not allowed to read, and permission misconfiguration is never reported.

Fix: the ListAvailableResourceMetrics handler in the pi service crate — specifically the ordering inside its request path. Resolve and authorize the (ServiceType, Identifier) pair against account-owned state FIRST and raise NotAuthorizedException when that fails; only then run the ServiceType/MetricTypes compatibility validation that currently raises InvalidArgumentException. The existing metric-type validation is not itself wrong, it is merely reached too early.

Traces: fixture pi-docdb-service-type, step task#3 (divergence_key service=pi|ListAvailableResourceMetrics|status=400->400|keys=$.Message,$.__type).

## Evidence
Fixture pi-docdb-service-type, step task#3, operation ListAvailableResourceMetrics, verdict=mismatch.
Expected status 400, actual status 400 (status matches; the body diverges).
$.__type: value_mismatch — expected 'NotAuthorizedException', got 'InvalidArgumentException'.
$.Message: value_mismatch — expected 'The specified resource is not authorized for this account', got 'The metric type "db" is not published for a DOCDB data source; the metric types published for it are os.' (recorded message cited as an example only; the error code is the requirement).
Both error shapes are modeled on this operation at httpError 400, so the error code is the only discriminator on the wire.
No related open issue covers this operation; the only open pi issue is 20261022T162222Z-pi-getresourcemetadata-stub.md (GetResourceMetadata unimplemented scaffold stub), a different operation.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. replay_wire_corpus() and look at fixture pi-docdb-service-type, step task#3.
3. Equivalently, by hand: call pi ListAvailableResourceMetrics with ServiceType=DOCDB, an Identifier for a resource not authorized for the calling account, and MetricTypes containing "db".
4. Observe HTTP 400 with __type=InvalidArgumentException complaining that metric type "db" is not published for a DOCDB data source.
5. Expected: HTTP 400 with __type=NotAuthorizedException (the authorization check must be evaluated before MetricTypes validation).
