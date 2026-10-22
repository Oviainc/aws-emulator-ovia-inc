Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-error-paths service-quotas-quota-change-history-and-support service-quotas-service-code-pattern-validation
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|GetServiceQuota|4xx->4xx|$.Message
Issue-Key: servicequotas|GetServiceQuota|4xx->4xx|status
Issue-Key: servicequotas|ListServiceQuotas|4xx->4xx|$.Message
Issue-Key: servicequotas|ListServiceQuotas|4xx->4xx|status
Issue-Key: servicequotas|GetRequestedServiceQuotaChange|4xx->4xx|$.Message
Issue-Key: servicequotas|GetRequestedServiceQuotaChange|4xx->4xx|status
Issue-Key: servicequotas|ListRequestedServiceQuotaChangeHistoryByQuota|4xx->4xx|$.Message
Issue-Key: servicequotas|ListRequestedServiceQuotaChangeHistoryByQuota|4xx->4xx|status
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Service Quotas error paths answer 404 instead of 400 and omit the Message member

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceQuota

## Description
Service Quotas' lookup failures are served on the wrong HTTP status, and its input-validation failure is served with no Message member at all. Five causes, all in the service's error mapping.

Cause 1 of 5 — GetServiceQuota for an unknown service/quota pair answers 404 instead of 400.
AWS: the recorded response for GetServiceQuota with a nonexistent ServiceCode+QuotaCode is HTTP 400 carrying NoSuchResourceException (example message: 'The request failed because the specified quota and service do not exist.'). Note the Smithy model annotates NoSuchResourceException with @httpError 404, but this is an awsJson1_1 service and the recorded wire status is 400 — the recording is the wire truth here, and the exception shape itself is unchanged.
Emulator: HTTP 404 with the same error shape and its own message ('no quota with ServiceCode ... and QuotaCode ... exists').
Why: SDK retry/error classification and any client branching on the HTTP status (4xx bucketing, API-gateway-style passthrough, status-based circuit breakers) sees a different status than AWS produces, so a 'not found' is handled on a path AWS never puts it on. The message text itself is an example, not the requirement; the status is.
Fix: the error-to-HTTP-status mapping for NoSuchResourceException in the Service Quotas serializer.
Traces: service-quotas-error-paths#task#1.

Cause 2 of 5 — ListServiceQuotas for an unknown service code answers 404 instead of 400.
AWS: the recorded response for ListServiceQuotas with a nonexistent ServiceCode is HTTP 400 (example message: 'This service is not available in the current Region. Choose a different Region or a different service. ').
Emulator: HTTP 404 (message 'no service quotas are available for ServiceCode ...').
Why: as cause 1 — status-based handling diverges. Note also that AWS frames an unknown service code as a region/availability condition, not a bare not-found.
Fix: the same error-to-HTTP-status mapping, on the ListServiceQuotas path.
Traces: service-quotas-error-paths#task#3.

Cause 3 of 5 — GetRequestedServiceQuotaChange for an unknown RequestId answers 404 instead of 400.
AWS: recorded response is HTTP 400 (example message: 'The request failed because the specified requestId to increase the service quota does not exist.'). Smithy: GetRequestedServiceQuotaChangeRequest.RequestId @required, @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$, errors include NoSuchResourceException.
Emulator: HTTP 404.
Why: as cause 1, on the quota-increase-request read path, which is the path a client polls after RequestServiceQuotaIncrease.
Fix: the same error-to-HTTP-status mapping, on the GetRequestedServiceQuotaChange path.
Traces: service-quotas-error-paths#task#6.

Cause 4 of 5 — ListRequestedServiceQuotaChangeHistoryByQuota for an unknown service code answers 404 instead of 400.
AWS: recorded response is HTTP 400 (example message: 'This service is not available in the current Region. Choose a different Region or a different service. ').
Emulator: HTTP 404 (message 'no quota with ServiceCode ... and QuotaCode ... exists').
Why: as cause 1, on the change-history path.
Fix: the same error-to-HTTP-status mapping, on the ListRequestedServiceQuotaChangeHistoryByQuota path.
Traces: service-quotas-quota-change-history-and-support#task#1.

Cause 5 of 5 — the pattern-validation rejection is returned with NO Message member.
AWS: QuotaCode is constrained by @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$ (Smithy: GetServiceQuotaRequest.QuotaCode, @length 1-128), and a value violating it is rejected with HTTP 400 whose body carries a Message built from the request — it names the member and the constraint it failed, in the standard AWS form 'QuotaCode failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9-]{1,128}'. That construction rule is the requirement, not the exact recorded string. Smithy: ExceptionMessage member 'Message' on IllegalArgumentException/NoSuchResourceException.
Emulator: correct status (400) but $.Message is absent from the body entirely.
Why: the Message member is the only machine-independent description of which input the caller got wrong. A client (or a human reading an SDK exception) that logs or surfaces err.message gets an empty string and cannot tell which of the two constrained members was rejected; SDK error stringification produces a message-less exception.
Fix: the Service Quotas input-validation layer — it must populate the exception's Message with the offending member name and the constraint it violated.
Traces: service-quotas-error-paths#task#2 (also affects service-quotas-service-code-pattern-validation#task#1 in the same cluster).

## Evidence
Cause 1: fixture service-quotas-error-paths step task#1, GetServiceQuota — status expected 400, got 404; $.Message expected 'The request failed because the specified quota and service do not exist.', got "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists" (message quoted as an example; the status is the requirement).
Cause 2: fixture service-quotas-error-paths step task#3, ListServiceQuotas — status expected 400, got 404; $.Message expected 'This service is not available in the current Region. Choose a different Region or a different service. ', got "no service quotas are available for ServiceCode 'not-a-real-service-xyz'".
Cause 3: fixture service-quotas-error-paths step task#6, GetRequestedServiceQuotaChange — status expected 400, got 404; $.Message expected 'The request failed because the specified requestId to increase the service quota does not exist.', got "no quota increase request with RequestId '00000000-0000-0000-0000-000000000000' exists".
Cause 4: fixture service-quotas-quota-change-history-and-support step task#1, ListRequestedServiceQuotaChangeHistoryByQuota — status expected 400, got 404; $.Message expected 'This service is not available in the current Region. ...', got "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-00000000' exists".
Cause 5: fixture service-quotas-error-paths step task#2, GetServiceQuota — status 400 on both sides; $.Message expected 'QuotaCode failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9-]{1,128}', got None (member absent). Same cluster: service-quotas-service-code-pattern-validation#task#1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: POST GetServiceQuota with ServiceCode 'not-a-real-service-xyz', QuotaCode 'L-00000000' -> 404; AWS returns 400. Fixture service-quotas-error-paths, step task#1.
3. Cause 2: POST ListServiceQuotas with ServiceCode 'not-a-real-service-xyz' -> 404; AWS returns 400. Fixture service-quotas-error-paths, step task#3.
4. Cause 3: POST GetRequestedServiceQuotaChange with RequestId '00000000-0000-0000-0000-000000000000' -> 404; AWS returns 400. Fixture service-quotas-error-paths, step task#6.
5. Cause 4: POST ListRequestedServiceQuotaChangeHistoryByQuota with ServiceCode 'not-a-real-service-xyz', QuotaCode 'L-00000000' -> 404; AWS returns 400. Fixture service-quotas-quota-change-history-and-support, step task#1.
6. Cause 5: POST GetServiceQuota with a QuotaCode that violates ^[a-zA-Z][a-zA-Z0-9-]{1,128}$ -> 400 with no Message member in the body. Fixture service-quotas-error-paths, step task#2.
