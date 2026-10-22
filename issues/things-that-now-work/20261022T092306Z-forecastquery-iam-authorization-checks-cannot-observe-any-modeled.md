Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: forecastquery.query-forecast-missing-iam-permission-denied forecastquery.query-whatif-forecast-missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.forecastquery#AmazonForecastRuntime
Issue-Key: forecastquery.query-forecast-missing-iam-permission-denied|run#14|AssertionError|assert err_admin["Code"] == "ResourceNotFoundException", (
Issue-Key: forecastquery.query-whatif-forecast-missing-iam-permission-denied|run#15|AssertionError|assert err_privileged["Code"] == "InvalidInputException", (
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# forecastquery IAM-authorization checks cannot observe any modeled outcome: router refuses QueryForecast/QueryWhatIfForecast with non-AWS 'AmbiguousService' instead of dispatching

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryForecast

## Description
Single root cause (Cause 1 of 1) behind the two remaining non-owned forecastquery behavioral failures (forecastquery.query-forecast-missing-iam-permission-denied, forecastquery.query-whatif-forecast-missing-iam-permission-denied). Both checks fail on their positive control: a fully-privileged caller's QueryForecast / QueryWhatIfForecast request is refused by the emulator's request router before it reaches any forecastquery handler, so neither the IAM-denial path nor any modeled error path of this service can be observed.

Cause 1 of 1: service resolution keys on the SigV4 signing name, which is not unique across services. forecastquery and forecast both declare sigv4Name=forecast, so the router finds two candidates and refuses the request instead of dispatching on a discriminator that IS unique.
Failing check ids for this cause: forecastquery.query-forecast-missing-iam-permission-denied, forecastquery.query-whatif-forecast-missing-iam-permission-denied.

AWS: forecastquery is a distinct wire service from forecast. Smithy (services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json): service AmazonForecastRuntime, awsJson1_1, sdkId=forecastquery, endpointPrefix=forecastquery, arnNamespace=forecast, sigv4Name=forecast; the control plane (services/service-com-amazonaws-forecast-amazonforecast/smithy-model/...json) declares sdkId=forecast, endpointPrefix=forecast, sigv4Name=forecast. The signing name is deliberately shared; the endpoint prefix and the awsJson1_1 X-Amz-Target service prefix (AmazonForecastRuntime.<Op> vs AmazonForecast.<Op>) are what distinguish the two. Smithy: QueryForecast input ForecastArn required, @length max 256, pattern ^arn:([a-z\d-]+):forecast:.*:.*:.+$; Filters required, 1-50 entries; NextToken @length 1-3000; modeled errors InvalidInputException (400), InvalidNextTokenException (400), LimitExceededException (409), ResourceInUseException (409), ResourceNotFoundException (404), else 200 QueryForecastResponse. QueryWhatIfForecast is identical with WhatIfForecastArn (@length max 300) and the same error set. AmbiguousService is not a shape in either model and is not an AWS error code. For the IAM dimension these checks exercise: recorded behavior only — a well-formed-but-absent ForecastArn called with full privileges reaches resource resolution and yields ResourceNotFoundException (404), while a principal with zero IAM grants is refused with AccessDeniedException (QueryForecast); for QueryWhatIfForecast the recording shows the fabricated-ARN rejection InvalidInputException (400) dominating for both the privileged and the zero-grants principal. The requirement these checks defend is that an unauthorized principal NEVER receives a 200 or any forecast data.

Emulator: both calls return Code 'AmbiguousService' with the message (recorded example, not a requirement) "2 emulated services all claim requests for 'forecast': forecast, forecast; the request is rejected rather than served by an arbitrary one". The refusal is emitted for the privileged caller before the request body is parsed, so the restricted-principal leg never runs and no authorization decision is made at all.

Why: a client relying on AWS behavior gets a non-modeled error code for 100% of forecastquery traffic — `except fq.exceptions.ResourceNotFoundException` / `InvalidInputException` / `AccessDeniedException` never match, retry classification is wrong, and the Forecast runtime/data plane is unusable. Worse for this cluster: because the router answers before authorization, the emulator provides no evidence that an unauthorized principal is denied; once dispatch is fixed, the authorization outcome is still unverified and could fail open.

Fix: the shared request-router / service-resolution component of the gateway (not the forecastquery operation handlers) must disambiguate colliding sigv4 signing names using a per-service-unique discriminator — the request host/endpoint prefix (forecastquery vs forecast) and, for awsJson1_1, the X-Amz-Target service prefix — and refuse only when no discriminator separates candidates. After that, the QueryForecast / QueryWhatIfForecast handlers, their input validation and the service's IAM authorization path must produce the modeled outcomes above.

Related open issue: 20261022T062300Z-forecastquery-is-entirely-unreachable-router-refuses-every-request.md describes the same router collision but carries only the other 11 forecastquery check ids in its replay key; these two IAM-authorization checks are not covered by it and additionally require the authorization leg to be verified.

Traces: forecastquery.query-forecast-missing-iam-permission-denied (assertion at check line 53), forecastquery.query-whatif-forecast-missing-iam-permission-denied (assertion at check line 62).

## Evidence
Suite: forecastquery 0/13 (0%). Two failures not owned by any open issue:

1) forecastquery.query-forecast-missing-iam-permission-denied
   Call: query_forecast(ForecastArn="arn:aws:forecast:<region>:<account>:forecast/sim2real-any-thing", Filters={"item_id": "sim2real-item"}) with the account's own fully-privileged credentials.
   Expected: Error.Code == 'ResourceNotFoundException' (HTTP 404) — positive control proving service/region/account are healthy; then the same call with a brand-new IAM user holding zero policies must yield Error.Code == 'AccessDeniedException'.
   Actual: {'Code': 'AmbiguousService', 'Message': "2 emulated services all claim requests for 'forecast': forecast, forecast; the request is rejected rather than served by an arbitrary one"} (message quoted as a recorded example; the diverging requirement is the code/shape and status). The restricted-principal leg was never reached.

2) forecastquery.query-whatif-forecast-missing-iam-permission-denied
   Call: query_whatif_forecast(WhatIfForecastArn=<fabricated arn>, Filters={...}) with fully-privileged credentials.
   Expected: Error.Code == 'InvalidInputException' (HTTP 400) for the privileged caller, and the zero-grants principal must receive the same rejection rather than any 200/forecast data.
   Actual: {'Code': 'AmbiguousService', ...} — same router refusal.

Model evidence of the collision: forecastquery sigv4Name=forecast, endpointPrefix=forecastquery; forecast sigv4Name=forecast, endpointPrefix=forecast. 'AmbiguousService' is not a modeled shape in either model.

## Steps to Reproduce
1. git_clone the repo and boot the emulator (both service-com-amazonaws-forecast-amazonforecast and service-com-amazonaws-forecastquery-amazonforecastruntime are bootstrapped, so both register the 'forecast' signing name).
2. Create a botocore client for 'forecastquery' against the emulator endpoint.
3. With the account's default (privileged) credentials call query_forecast(ForecastArn="arn:aws:forecast:<region>:<account>:forecast/sim2real-any-thing", Filters={"item_id": "sim2real-item"}).
   Observed: ClientError Error.Code == 'AmbiguousService'.
   Expected: Error.Code == 'ResourceNotFoundException', HTTP 404.
4. Create a new IAM user with NO attached policies, mint an access key, and build a second forecastquery client with those credentials; repeat the call.
   Expected: Error.Code == 'AccessDeniedException'; it must never return 200 or forecast data.
5. Repeat steps 3-4 with query_whatif_forecast(WhatIfForecastArn=<fabricated arn>, Filters={...}); expected Error.Code == 'InvalidInputException' (HTTP 400) for both principals.
6. Replay the two check ids in trace_ids to confirm the fix.
