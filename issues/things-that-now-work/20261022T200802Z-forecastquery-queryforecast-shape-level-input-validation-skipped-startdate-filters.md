Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: forecastquery.query-forecast-date-format-validation-rejected forecastquery.query-forecast-filter-key-invalid-characters-rejected forecastquery.query-forecast-end-date-before-start-date-not-validated forecastquery.query-forecast-resource-resolution-precedes-next-token-validation forecastquery.query-forecast-missing-iam-permission-denied forecastquery.query-forecast-arn-validation-precedes-lookup
Issue-Shape-Id: com.amazonaws.forecastquery#AmazonForecastRuntime
Issue-Key: forecastquery.query-forecast-date-format-validation-rejected|run#8|AssertionError|assert err_bad["Code"] == "ValidationException", (
Issue-Key: forecastquery.query-forecast-filter-key-invalid-characters-rejected|run#7|AssertionError|assert err_bad["Code"] == "ValidationException", (
Issue-Key: forecastquery.query-forecast-end-date-before-start-date-not-validated|run#9|AssertionError|assert err_reversed["Message"] == expected_message, (
Issue-Key: forecastquery.query-forecast-resource-resolution-precedes-next-token-validation|run#9|AssertionError|assert err_bogus["Message"] == not_found, (
Issue-Key: forecastquery.query-forecast-missing-iam-permission-denied|run#15|AssertionError|assert err_admin["Message"] == f"No resource found {some_arn}", (
Issue-Key: forecastquery.query-forecast-arn-validation-precedes-lookup|run#7|AssertionError|assert err_malformed["Message"] == "Invalid ARN for forecast: definitely-not-an-arn", (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# QueryForecast: shape-level input validation skipped (StartDate, Filters keys) and error messages diverge from AWS

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryForecast

## Description
The forecastquery emulator answers QueryForecast, but its validation pipeline and its error messages diverge from real AWS in four independent ways. All four are in the QueryForecast request path of the forecastquery service.

Cause 1 of 4 — StartDate/EndDate format is never validated, so a malformed date falls through to resource resolution.
AWS: QueryForecast.StartDate/EndDate target the `DateTime` string (Smithy: com.amazonaws.forecastquery#DateTime, no pattern in the on-disk model, so recorded behavior only). The recording shows real AWS performing shape-level validation of the date format and rejecting a value that is not ISO 8601 (yyyy-MM-dd'T'HH:mm:ss) with HTTP 400 `ValidationException`, message built from the request: `1 validation error detected: Value '<value>' at 'startDate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}[a-zA-Z0-9\:\+\-\[\]\.\/]*$`. Validation precedes ARN resolution, so this is returned even when the ForecastArn does not exist.
Emulator: StartDate='2024/01/01 00:00:00' is accepted by the validator and the request proceeds to resource lookup, which answers `ResourceNotFoundException` (404) instead.
Why: a client that relies on date-format rejection sees a 404 resource error for what is really a client-side input bug; retry/branching logic keyed on ValidationException never fires, and callers cannot distinguish a bad date literal from a genuinely missing forecast.
Fix: QueryForecast input validation (the date-format check must run, and run before ARN resolution).
Traces: forecastquery.query-forecast-date-format-validation-rejected.

Cause 2 of 4 — Filters map keys are never validated against the modeled key pattern.
AWS: Smithy: QueryForecast.Filters targets com.amazonaws.forecastquery#Filters, whose key targets `AttributeName` with @length min 0 max 256 and @pattern `^[a-zA-Z0-9\_\-]+$`. A key violating that pattern is rejected at the shape-validation layer with HTTP 400 `ValidationException` (code per recording — NOT the modeled InvalidInputException), message built from the request and naming the member and the violated constraints, e.g. `1 validation error detected: Value '{item id!=sim2real-item}' at 'filters' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ^[a-zA-Z0-9\_\-]+$]`. This also precedes ARN resolution.
Emulator: a filter key containing a space and '!' is accepted and the request proceeds to resource lookup, answering `ResourceNotFoundException` (404).
Why: invalid filter keys are silently tolerated, so a client sees a resource error (or, once a forecast exists, a query that quietly matches nothing) rather than the input rejection AWS gives.
Fix: QueryForecast input validation — enforce the Filters key @pattern/@length from the model before resource resolution.
Traces: forecastquery.query-forecast-filter-key-invalid-characters-rejected.

Cause 3 of 4 — ResourceNotFoundException message does not follow the AWS rule.
AWS: for a well-formed but absent ForecastArn, QueryForecast returns HTTP 404 `ResourceNotFoundException` (Smithy: ResourceNotFoundException, httpError 404) whose message is composed from the request as `No resource found <the ForecastArn supplied>` — the rule is request-derived and is observed identically across every recorded branch that reaches resource resolution.
Emulator: returns the correct code/status but a message of the form `We can't find that resource. No forecast with ARN <arn> exists.` (the Smithy doc string plus emulator-specific prose).
Why: clients and test suites that match on the documented message shape, or that parse the ARN back out of it, break even though the code is right.
Fix: QueryForecast ResourceNotFoundException construction (error serialization for the not-found path).
Traces: forecastquery.query-forecast-end-date-before-start-date-not-validated, forecastquery.query-forecast-resource-resolution-precedes-next-token-validation, forecastquery.query-forecast-missing-iam-permission-denied.

Cause 4 of 4 — malformed-ForecastArn InvalidInputException message does not follow the AWS rule.
AWS: Smithy: QueryForecast.ForecastArn is required, targets `Arn` with @length 0-256 and @pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`; a violation yields HTTP 400 `InvalidInputException` (modeled, httpError 400). The recorded message is composed from the request as `Invalid ARN for forecast: <the value supplied>`, and that same rule holds whether or not other fields (e.g. NextToken) are also bad — ARN-shape validation dominates.
Emulator: returns the correct code/status with a generic shape-validation message instead: `The value is invalid or is too long. Value '<value>' at 'forecastArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\d-]+):forecast:.*:.*:.+$`.
Why: the ARN-rejection message a client surfaces or matches on differs from AWS for every malformed-ARN call.
Fix: QueryForecast ForecastArn validation / InvalidInputException construction.
Traces: forecastquery.query-forecast-arn-validation-precedes-lookup, forecastquery.query-forecast-resource-resolution-precedes-next-token-validation.

## Evidence
Check forecastquery.query-forecast-date-format-validation-rejected — expected: Code=ValidationException for StartDate='2024/01/01 00:00:00'; actual: {'Code': 'ResourceNotFoundException', 'Message': "We can't find that resource. No forecast with ARN arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_c745503142dc exists."}

Check forecastquery.query-forecast-filter-key-invalid-characters-rejected — expected: Code=ValidationException for Filters={'item id!': 'sim2real-item'}; actual: {'Code': 'ResourceNotFoundException', 'Message': "We can't find that resource. No forecast with ARN arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_4c3ebd6381d6 exists."}

Checks forecastquery.query-forecast-end-date-before-start-date-not-validated / query-forecast-resource-resolution-precedes-next-token-validation / query-forecast-missing-iam-permission-denied — code ResourceNotFoundException matches, message does not. Expected (AWS rule, example): 'No resource found arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_480d717fe854'; actual: "We can't find that resource. No forecast with ARN arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_480d717fe854 exists."

Check forecastquery.query-forecast-arn-validation-precedes-lookup — code InvalidInputException matches. Expected (AWS rule, example): 'Invalid ARN for forecast: definitely-not-an-arn'; actual: "The value is invalid or is too long. Value 'definitely-not-an-arn' at 'forecastArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\\d-]+):forecast:.*:.*:.+$"

## Steps to Reproduce
1. Point a botocore forecastquery client at the emulator (region us-west-2).
2. Cause 1: query_forecast(ForecastArn='arn:aws:forecast:us-west-2:<acct>:forecast/does_not_exist', Filters={'item_id':'x'}, StartDate='2024/01/01 00:00:00') -> emulator: 404 ResourceNotFoundException; AWS: 400 ValidationException naming 'startDate'.
3. Cause 2: query_forecast(ForecastArn=<same absent ARN>, Filters={'item id!': 'x'}) -> emulator: 404 ResourceNotFoundException; AWS: 400 ValidationException naming 'filters' map-key constraints.
4. Cause 3: query_forecast(ForecastArn=<same absent ARN>, Filters={'item_id':'x'}) -> message is "We can't find that resource. No forecast with ARN <arn> exists." instead of 'No resource found <arn>'.
5. Cause 4: query_forecast(ForecastArn='definitely-not-an-arn', Filters={'item_id':'x'}) -> InvalidInputException whose message is the generic pattern-constraint text instead of 'Invalid ARN for forecast: definitely-not-an-arn'.
