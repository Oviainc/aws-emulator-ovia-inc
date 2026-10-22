Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: forecastquery.query-forecast-arn-validation-precedes-lookup forecastquery.query-forecast-concurrent-requests-isolated forecastquery.query-forecast-date-format-validation-rejected forecastquery.query-forecast-end-date-before-start-date-not-validated forecastquery.query-forecast-filter-key-invalid-characters-rejected forecastquery.query-forecast-resource-resolution-precedes-next-token-validation forecastquery.query-whatif-forecast-arn-validation-precedes-lookup forecastquery.query-whatif-forecast-date-format-validation-rejected forecastquery.query-whatif-forecast-end-date-before-start-date-rejected forecastquery.query-whatif-forecast-filter-key-invalid-characters-rejected forecastquery.query-whatif-forecast-invalid-next-token-rejected
Issue-Shape-Id: com.amazonaws.forecastquery#AmazonForecastRuntime
Issue-Key: forecastquery.query-forecast-arn-validation-precedes-lookup|run#6|AssertionError|assert err_malformed["Code"] == "InvalidInputException", (
Issue-Key: forecastquery.query-forecast-concurrent-requests-isolated|run#10|AssertionError|assert not mismatches, f"concurrent requests got mismatched error codes: {mismatches}"
Issue-Key: forecastquery.query-forecast-date-format-validation-rejected|run#8|AssertionError|assert err_bad["Code"] == "ValidationException", (
Issue-Key: forecastquery.query-forecast-end-date-before-start-date-not-validated|run#8|AssertionError|assert err_reversed["Code"] == "ResourceNotFoundException", (
Issue-Key: forecastquery.query-forecast-filter-key-invalid-characters-rejected|run#7|AssertionError|assert err_bad["Code"] == "ValidationException", (
Issue-Key: forecastquery.query-forecast-resource-resolution-precedes-next-token-validation|run#8|AssertionError|assert err_bogus["Code"] == "ResourceNotFoundException", (
Issue-Key: forecastquery.query-whatif-forecast-arn-validation-precedes-lookup|run#6|AssertionError|assert err_malformed["Code"] == "InvalidInputException", (
Issue-Key: forecastquery.query-whatif-forecast-date-format-validation-rejected|run#8|AssertionError|assert err_bad["Code"] == "InvalidInputException", (
Issue-Key: forecastquery.query-whatif-forecast-end-date-before-start-date-rejected|run#8|AssertionError|assert err_reversed["Code"] == "InvalidInputException", (
Issue-Key: forecastquery.query-whatif-forecast-filter-key-invalid-characters-rejected|run#7|AssertionError|assert code_bad == "InvalidInputException", (
Issue-Key: forecastquery.query-whatif-forecast-invalid-next-token-rejected|run#8|AssertionError|assert err_bogus["Code"] == "InvalidInputException", (
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# forecastquery is entirely unreachable: router refuses every request with AmbiguousService because forecast and forecastquery share the 'forecast' sigv4 signing name

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryForecast

## Description
Single root cause (Cause 1 of 1) behind all 11 non-deferred forecastquery behavioral failures. Every QueryForecast / QueryWhatIfForecast request — valid or invalid — is refused by the emulator's request router before it ever reaches a forecastquery handler, so no operation and no error path of this service is reachable.

Cause 1 of 1: service resolution keys on the SigV4 signing name, which is not unique across services, so AmazonForecast and AmazonForecastRuntime collide and the router refuses rather than dispatching.

AWS: `forecastquery` is a distinct wire service from `forecast`. Smithy (services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json): service AmazonForecastRuntime, version 2018-06-26, protocol awsJson1_1, awsService.sdkId=forecastquery, awsService.endpointPrefix=forecastquery, arnNamespace=forecast, sigv4Name=forecast. The control-plane model (services/service-com-amazonaws-forecast-amazonforecast/smithy-model/com-amazonaws-forecast-amazonforecast.json): service AmazonForecast, sdkId=forecast, endpointPrefix=forecast, arnNamespace=forecast, sigv4Name=forecast. So the signing name `forecast` is deliberately SHARED by the two services, while the endpoint prefix (forecastquery vs forecast) and the awsJson1_1 `X-Amz-Target` prefix (AmazonForecastRuntime.<Op> vs AmazonForecast.<Op>) are what distinguish them. Real AWS routes on host/endpoint and target, so a signed QueryForecast call is served and returns one of the modeled responses: QueryForecast/QueryWhatIfForecast errors are InvalidInputException (400), InvalidNextTokenException (400), LimitExceededException (409), ResourceInUseException (409), ResourceNotFoundException (404), or a 200 QueryForecastResponse. `AmbiguousService` is not a shape in either model and is not an AWS error code.

Emulator: every call returns the error code `AmbiguousService` with the message (recorded example, not a requirement) "2 emulated services all claim requests for 'forecast': forecast, forecast; the request is rejected rather than served by an arbitrary one". This is a router refusal, emitted identically for a syntactically malformed ForecastArn, a well-formed-but-absent ARN, a malformed StartDate/EndDate, an invalid filter key, and a bogus NextToken — i.e. the request body is never parsed and no forecastquery handler runs. The concurrency check shows the same single code for all 8 concurrent, per-thread-distinct inputs, confirming the refusal happens before any input-specific logic.

Why: a client relying on AWS behavior gets a non-AWS error code with no modeled error shape for 100% of forecastquery traffic. botocore/SDK error handling (`except fq.exceptions.ResourceNotFoundException`, `except InvalidInputException`) never matches, retry/backoff classification is wrong, and the whole runtime/data plane of Forecast is unusable against the emulator. Error-precedence semantics that real AWS exhibits (ARN syntax validation before resource resolution; resource resolution before NextToken validation; no validation of EndDate-before-StartDate ordering) cannot be observed at all.

Fix: the shared request-router / service-resolution component of the gateway, not the forecastquery operation handlers. Resolution must disambiguate colliding sigv4 signing names using a discriminator that is unique per service — the request host/endpoint prefix (forecastquery vs forecast) and, for awsJson1_1, the `X-Amz-Target` service prefix (AmazonForecastRuntime vs AmazonForecast) — and only refuse when no discriminator separates the candidates. Once dispatch reaches the service, the QueryForecast and QueryWhatIfForecast handlers and their input validation (ForecastArn pattern ^arn:([a-z\d-]+):forecast:.*:.*:.+$, max length 256; WhatIfForecastArn same pattern, max length 300; Filters required, 1–50 entries; NextToken length 1–3000) must produce the modeled errors above.

Traces: all 11 failing checks listed in trace_ids. Representative: forecastquery.query-forecast-arn-validation-precedes-lookup expected Code=InvalidInputException for a malformed ARN and ResourceNotFoundException for a well-formed absent ARN, got Code=AmbiguousService for both; forecastquery.query-forecast-concurrent-requests-isolated expected per-input InvalidInputException/ResourceNotFoundException across 8 threads, got AmbiguousService for all 8. Note: this is the same router defect class as open issues 20261022T031220Z-wisdom-..., 20261022T032542Z-application-autoscaling-lambda-..., 20261022T035032Z-events-..., 20261022T042904Z-chime-sdk-identity-...; none of those names forecast/forecastquery, so this collision pair is not covered by any open issue.

## Evidence
Suite: forecastquery 0/13 (0%). 11 mismatches, all with the same emulator response.

Expected (AWS, per Smithy model + check assertions):
- QueryForecast(ForecastArn="definitely-not-an-arn", Filters={...}) -> 400 InvalidInputException
- QueryForecast(ForecastArn="arn:aws:forecast:us-west-2:<acct>:forecast/<absent>") -> 404 ResourceNotFoundException
- QueryForecast with malformed StartDate -> client validation rejection (ValidationException/InvalidInputException per check)
- QueryForecast with invalid filter-key characters -> client validation rejection
- QueryForecast(absent ARN + bogus NextToken) -> 404 ResourceNotFoundException (ARN resolution precedes token validation)
- QueryWhatIfForecast, same precedence with InvalidInputException dominating
- QueryForecast(EndDate < StartDate) -> NOT a validation error; ResourceNotFoundException (date ordering is not validated by real AWS)

Actual (emulator), identical for every one of the above:
  {'Code': 'AmbiguousService', 'Message': "2 emulated services all claim requests for 'forecast': forecast, forecast; the request is rejected rather than served by an arbitrary one"}
(message quoted as a recorded example; the diverging requirement is the error CODE/shape — AmbiguousService is not a modeled shape and no modeled error or success response is ever produced)

Concurrency evidence that the refusal precedes all input handling — 8 concurrent calls with 8 distinct inputs, each expecting its own code, all got AmbiguousService:
  ('sim2real-malformed-arn-0-not-a-real-arn', expected 'InvalidInputException', got 'AmbiguousService')
  ('arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_concurrent_1', expected 'ResourceNotFoundException', got 'AmbiguousService')
  ... (same for indices 2-7)

Model evidence of the collision: forecastquery sigv4Name=forecast, endpointPrefix=forecastquery; forecast sigv4Name=forecast, endpointPrefix=forecast.

## Steps to Reproduce
1. git_clone the repo and boot the emulator (both service-com-amazonaws-forecast-amazonforecast and service-com-amazonaws-forecastquery-amazonforecastruntime are bootstrapped, so both register the 'forecast' signing name).
2. Create a botocore client for 'forecastquery' against the emulator endpoint in us-west-2.
3. Call query_forecast(ForecastArn="definitely-not-an-arn", Filters={"item_id": "sim2real-item"}).
   Observed: ClientError with Error.Code == 'AmbiguousService'.
   Expected: Error.Code == 'InvalidInputException' (HTTP 400).
4. Call query_forecast(ForecastArn="arn:aws:forecast:us-west-2:<account>:forecast/does_not_exist", Filters={"item_id": "sim2real-item"}).
   Observed: Error.Code == 'AmbiguousService'.
   Expected: Error.Code == 'ResourceNotFoundException' (HTTP 404).
5. Repeat with query_whatif_forecast(WhatIfForecastArn=...) — same AmbiguousService refusal.
6. Replay the 11 check ids in trace_ids to confirm the fix.
