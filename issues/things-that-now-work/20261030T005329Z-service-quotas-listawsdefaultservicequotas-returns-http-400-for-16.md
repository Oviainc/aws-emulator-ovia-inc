Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-signin
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-signin service-quotas-catalog-list-aws-default-service-quotas-sms service-quotas-catalog-list-aws-default-service-quotas-sms-voice service-quotas-catalog-list-aws-default-service-quotas-snow-device-management service-quotas-catalog-list-aws-default-service-quotas-snowball service-quotas-catalog-list-aws-default-service-quotas-social-messaging service-quotas-catalog-list-aws-default-service-quotas-support service-quotas-catalog-list-aws-default-service-quotas-supportauthz service-quotas-catalog-list-aws-default-service-quotas-thinclient service-quotas-catalog-list-aws-default-service-quotas-timestream-influxdb service-quotas-catalog-list-aws-default-service-quotas-tnb service-quotas-catalog-list-aws-default-service-quotas-ts service-quotas-catalog-list-aws-default-service-quotas-vmimportexport service-quotas-catalog-list-aws-default-service-quotas-voiceid service-quotas-catalog-list-aws-default-service-quotas-wam service-quotas-catalog-list-aws-default-service-quotas-workspaces-instances
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->4xx|$.Quotas
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->4xx|status
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# Service Quotas ListAWSDefaultServiceQuotas returns HTTP 400 for 16 valid ServiceCodes

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
Cause 1 of 1: ListAWSDefaultServiceQuotas rejects 16 real AWS ServiceCodes with HTTP 400 instead of returning their default quota catalog. Representative: fixture service-quotas-catalog-list-aws-default-service-quotas-signin, step task#1. The same cluster covers the ServiceCodes signin, sms, sms-voice, snow-device-management, snowball, social-messaging, support, supportauthz, thinclient, timestream-influxdb, tnb, ts, vmimportexport, voiceid, wam and workspaces-instances.

AWS: ListAWSDefaultServiceQuotas(ServiceCode=<code>) answers 200 with the `Quotas` list for that service. ServiceCode is modeled @required with @length 1..63 and @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$ (Smithy: ListAWSDefaultServiceQuotasRequest.ServiceCode), and all sixteen codes above satisfy it and are codes Service Quotas publishes, so the recorded responses are 200 with a non-empty Quotas list (for example signin returns the entries L-42359A23 'Maximum Signin resource based policy statement size' Unit=Bytes Value=10240, Adjustable=false, GlobalQuota=false, QuotaArn arn:aws:servicequotas:<region>::signin/L-42359A23 and L-5ECCC143 Value=100). The operation's only modeled client failures are AccessDeniedException (403), IllegalArgumentException (400), InvalidPaginationTokenException (400), NoSuchResourceException (404) and TooManyRequestsException (429); a ServiceCode the service does not know is NoSuchResourceException with HTTP 404, never a 400, and a well-formed code that AWS does publish is not an error at all.

Emulator: the step returns HTTP 400 and no body member Quotas at all (replay detail: 'status: expected 200, got 400' and '$.Quotas: missing'). Any ServiceCode outside the emulator's hand-written table is turned into a 400 client error.

Why: a client enumerating service quotas (console-style sweeps, capacity-planning and compliance tooling, anything that walks ListServices then ListAWSDefaultServiceQuotas per service) sees a hard client error on sixteen services instead of data. Because the status is 400, SDK retry policy treats it as a non-retryable caller mistake and the caller aborts rather than skipping the service; and because AWS signals an unknown resource with NoSuchResourceException/404, code that branches on the error class mis-handles it. The error class, HTTP status and message are requirements: for a code the emulator cannot serve the correct answer is the modeled 200 catalog, and the only acceptable failure for an unknown ServiceCode is NoSuchResourceException with HTTP 404 and AWS's message text for that error class, whose variable part is the ServiceCode taken from the request.

Fix: the ListAWSDefaultServiceQuotas handler and its quota catalog state for Service Quotas — the catalog must cover every ServiceCode the service publishes (these sixteen included) and the handler must not map 'code absent from my table' onto a 400; an unknown code belongs on NoSuchResourceException/404.

Traces: service-quotas-catalog-list-aws-default-service-quotas-signin#task#1 (representative; the cluster also contains the sms, sms-voice, snow-device-management, snowball, social-messaging, support, supportauthz, thinclient, timestream-influxdb, tnb, ts, vmimportexport, voiceid, wam and workspaces-instances fixtures, same step task#1).

Related: the open issue 20261029T192055Z-service-quotas-listawsdefaultservicequotas-serves-a-hand-written-default-quota-table.md is about wrong catalog CONTENT on other ServiceCodes and carries no replay key for this 400 cluster, so it does not cover this fault.

## Evidence
Expected (recorded AWS response, fixture service-quotas-catalog-list-aws-default-service-quotas-signin step task#1): HTTP 200 with Quotas = [{ServiceCode: signin, ServiceName: 'AWS IAM Identity Center', QuotaCode: L-42359A23, QuotaArn: arn:aws:servicequotas:<region>::signin/L-42359A23, QuotaName: 'Maximum Signin resource based policy statement size', Unit: Bytes, Value: 10240, Adjustable: false, GlobalQuota: false, Description: 'The maximum size of individual Resource-Based Policy statements in bytes'}, {QuotaCode: L-5ECCC143, Value: 100, Unit: None, ...}].
Actual (emulator): HTTP 400, response carries no Quotas member ('$.Quotas: missing').
Same expected-200/actual-400 divergence on the 15 other ServiceCodes in the cluster (sms, sms-voice, snow-device-management, snowball, social-messaging, support, supportauthz, thinclient, timestream-influxdb, tnb, ts, vmimportexport, voiceid, wam, workspaces-instances).
Smithy: ListAWSDefaultServiceQuotasRequest.ServiceCode @required @length 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; errors AccessDeniedException(403), IllegalArgumentException(400), InvalidPaginationTokenException(400), NoSuchResourceException(404), ServiceException(500), TooManyRequestsException(429).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-signin, step task#1: call Service Quotas ListAWSDefaultServiceQuotas with {"ServiceCode": "signin"} against the emulator's data listener.
3. Observe HTTP 400 with no Quotas member; the recorded AWS response is HTTP 200 with the signin default-quota list.
4. Repeat with ServiceCode sms, sms-voice, snow-device-management, snowball, social-messaging, support, supportauthz, thinclient, timestream-influxdb, tnb, ts, vmimportexport, voiceid, wam and workspaces-instances (same fixture family, step task#1) — every one answers 400.
