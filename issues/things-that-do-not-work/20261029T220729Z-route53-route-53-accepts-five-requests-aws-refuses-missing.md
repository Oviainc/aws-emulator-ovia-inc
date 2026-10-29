Issue-Source: trace-sweep-divergence
Issue-Trace: route53-account-limits-and-reference-data
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-account-limits-and-reference-data route53-health-check-calculated-type-status-rejected route53-tcp-and-calculated-health-checks-isolation route53-health-check-error-paths route53-list-hosted-zones-by-name-reverse-label-order-and-paging
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|ListHostedZonesByVPC|4xx->2xx|$.ErrorResponse
Issue-Key: route53|ListHostedZonesByVPC|4xx->2xx|status
Issue-Key: route53|GetHealthCheckStatus|4xx->2xx|$.ErrorResponse
Issue-Key: route53|GetHealthCheckStatus|4xx->2xx|status
Issue-Key: route53|GetHealthCheckLastFailureReason|4xx->2xx|$.ErrorResponse
Issue-Key: route53|GetHealthCheckLastFailureReason|4xx->2xx|status
Issue-Key: route53|CreateHealthCheck|4xx->2xx|$.ErrorResponse
Issue-Key: route53|CreateHealthCheck|4xx->2xx|status
Issue-Key: route53|ListHostedZonesByName|4xx->2xx|$.ErrorResponse
Issue-Key: route53|ListHostedZonesByName|4xx->2xx|status
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 accepts five requests AWS refuses: missing authorization and input gates

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZonesByVPC

## Description
Five Route 53 request paths that AWS refuses are served as successes by the emulator. The shared fault is that the pre-flight gate (caller/VPC ownership, health-check-type applicability, request-element validation, DNS-name validation) is not run at all, so the handler falls through to the success path. 5 causes.

Cause 1 of 5 — ListHostedZonesByVPC, VPC not owned by the caller.
AWS: recorded behavior only (the Smithy model binds only InvalidInput and InvalidPaginationToken to ListHostedZonesByVPC, so the ownership refusal is not modeled). A request whose `vpcid` names a VPC that the calling account does not own is refused with HTTP 403 and a Route 53 `ErrorResponse` whose `Error` carries Type `Sender` and Code `AccessDenied`. The message is "The VPC: <vpcid> in region <vpcregion> that you provided is not owned by you." — the fixed text is "The VPC: ", " in region ", " that you provided is not owned by you."; the two variable parts are the `VPCId` and `VPCRegion` taken verbatim from the request query string.
Emulator: answers HTTP 200 with a ListHostedZonesByVPCResponse and no ErrorResponse element at all, so a foreign VPC is treated as an ownable one.
Why: a client enumerating zones for a VPC it does not own gets a silent empty/own-account answer instead of an authorization failure, so cross-account isolation is unobservable and code that branches on AccessDenied never takes that branch.
Fix: ListHostedZonesByVPC's handler — add the VPC-ownership pre-flight before the summary projection, emitting the 403 AccessDenied Sender error.
Traces: route53-account-limits-and-reference-data#task#8.

Cause 2 of 5 — GetHealthCheckStatus against a CALCULATED health check.
AWS: Smithy: GetHealthCheckStatus binds InvalidInput (httpError 400) and NoSuchHealthCheck (404); HealthCheckConfig.Type includes CALCULATED, documented as a health check whose state is computed from ChildHealthChecks. A status read on a CALCULATED check is refused with 400 InvalidInput, Type `Sender`, and the message "Invalid parameter : To get the status of a calculated health check, use either the Route 53 console or the CloudWatch console. The GetHealthCheckStatus action can't return the health of calculated health checks." — this message is entirely fixed text, with no request- or state-derived parts.
Emulator: answers HTTP 200 with a GetHealthCheckStatusResponse and no ErrorResponse, i.e. it fabricates checker observations for a check type that has none.
Why: callers cannot distinguish a calculated check (whose health must be read from CloudWatch) from an endpoint check, and get invented per-checker observations that correspond to no probe.
Fix: GetHealthCheckStatus's validation — gate on the stored HealthCheckConfig.Type before building observations.
Traces: route53-health-check-calculated-type-status-rejected#task#2 (cluster also covers route53-tcp-and-calculated-health-checks-isolation#task#3).

Cause 3 of 5 — GetHealthCheckLastFailureReason against a CALCULATED health check.
AWS: Smithy: GetHealthCheckLastFailureReason binds InvalidInput (400) and NoSuchHealthCheck (404). The same CALCULATED-type gate applies and AWS refuses with 400 InvalidInput carrying the identical fixed message, which names the GetHealthCheckStatus action even on this path: "Invalid parameter : To get the status of a calculated health check, use either the Route 53 console or the CloudWatch console. The GetHealthCheckStatus action can't return the health of calculated health checks." That cross-naming is part of the fixed text and must be reproduced as-is.
Emulator: answers HTTP 200 with a GetHealthCheckLastFailureReasonResponse and no ErrorResponse.
Why: same as cause 2 — a calculated check appears to expose endpoint failure reasons it cannot have.
Fix: GetHealthCheckLastFailureReason's validation — share the CALCULATED-type gate with GetHealthCheckStatus.
Traces: route53-health-check-calculated-type-status-rejected#task#3.

Cause 4 of 5 — CreateHealthCheck with a RequestInterval outside the accepted enumeration.
AWS: Smithy: HealthCheckConfig.RequestInterval is @range min 10 max 30, but the service accepts only the enumerated values 10 and 30, and enforces that during XML schema validation. A request carrying any other in-range integer is refused with 400 InvalidInput, Type `Sender`, message "Invalid XML ; javax.xml.stream.XMLStreamException: org.xml.sax.SAXParseException; lineNumber: 1; columnNumber: <n>; cvc-enumeration-valid: Value '<v>' is not facet-valid with respect to enumeration '[10, 30]'. It must be a value from the enumeration." — fixed text is the whole schema-violation sentence including the enumeration '[10, 30]'; the variable parts are the offending value <v> taken from the request and the <n> column offset of that element in the request body.
Emulator: answers HTTP 201 Created and stores the health check with the out-of-enumeration interval.
Why: a client can create a health check whose RequestInterval AWS would never persist, so the probe cadence it later reads back is unreachable on the real service and the refusal path is untestable.
Fix: CreateHealthCheck's validation — enforce the RequestInterval enumeration (not merely the 10..30 range) and render the schema-violation InvalidInput.
Traces: route53-health-check-error-paths#task#4.

Cause 5 of 5 — ListHostedZonesByName with an empty DNS label in `dnsname`.
AWS: Smithy: ListHostedZonesByName binds InvalidDomainName (httpError 400) and InvalidInput (400); DNSName is @length 0..1024 with no pattern, so label-level validation is service-side. A `dnsname` containing an empty label (two consecutive dots) is refused with 400 InvalidDomainName, Type `Sender`, message "FATAL problem: DomainLabelEmpty (Domain label is empty) encountered with '<dnsname>'" — fixed text is "FATAL problem: DomainLabelEmpty (Domain label is empty) encountered with '" and the closing quote; the variable part is the rejected dnsname echoed verbatim from the request.
Emulator: answers HTTP 200 with a ListHostedZonesByNameResponse and no ErrorResponse, treating the malformed name as a valid start-of-page key.
Why: paging clients that build `dnsname` from user input never learn the name is malformed; they instead receive a page positioned at an arbitrary point in the zone ordering.
Fix: ListHostedZonesByName's validation — run Route 53 DNS-name label validation on the `dnsname` query parameter before the reverse-label-order scan.
Traces: route53-list-hosted-zones-by-name-reverse-label-order-and-paging#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:
- route53-account-limits-and-reference-data#task#8 ListHostedZonesByVPC: status expected 403, got 200; $.ErrorResponse missing (expected Error{Type=Sender, Code=AccessDenied, Message='The VPC: <vpc-id> in region us-west-2 that you provided is not owned by you.'}, got None).
- route53-health-check-calculated-type-status-rejected#task#2 GetHealthCheckStatus: status expected 400, got 200; $.ErrorResponse missing (expected Error{Type=Sender, Code=InvalidInput, Message="Invalid parameter : To get the status of a calculated health check, use either the Route 53 console or the CloudWatch console. The GetHealthCheckStatus action can't return the health of calculated health checks."}). Cluster also covers route53-tcp-and-calculated-health-checks-isolation#task#3.
- route53-health-check-calculated-type-status-rejected#task#3 GetHealthCheckLastFailureReason: status expected 400, got 200; same expected InvalidInput body.
- route53-health-check-error-paths#task#4 CreateHealthCheck: status expected 400, got 201; $.ErrorResponse missing (expected Error{Type=Sender, Code=InvalidInput, Message="Invalid XML ; javax.xml.stream.XMLStreamException: org.xml.sax.SAXParseException; lineNumber: 1; columnNumber: <n>; cvc-enumeration-valid: Value '15' is not facet-valid with respect to enumeration '[10, 30]'. It must be a value from the enumeration."}).
- route53-list-hosted-zones-by-name-reverse-label-order-and-paging#task#7 ListHostedZonesByName: status expected 400, got 200; $.ErrorResponse missing (expected Error{Type=Sender, Code=InvalidDomainName, Message="FATAL problem: DomainLabelEmpty (Domain label is empty) encountered with '<dnsname>'"}).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the five steps named under Traces: each expects a 4xx ErrorResponse and the emulator answers 200/201 with a success envelope.
4. Individually: GET /2013-04-01/hostedzonesbyvpc?vpcid=<vpc not owned>&vpcregion=<region>; GET /2013-04-01/healthcheck/<calculated-check-id>/status; GET /2013-04-01/healthcheck/<calculated-check-id>/lastfailurereason; POST /2013-04-01/healthcheck with HealthCheckConfig.RequestInterval set to a value other than 10 or 30; GET /2013-04-01/hostedzonesbyname?dnsname=bad..<zone>.
