Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: forecastquery.query-whatif-forecast-arn-validation-precedes-lookup forecastquery.query-whatif-forecast-date-format-validation-rejected forecastquery.query-whatif-forecast-end-date-before-start-date-rejected forecastquery.query-whatif-forecast-filter-key-invalid-characters-rejected forecastquery.query-whatif-forecast-invalid-next-token-rejected forecastquery.query-whatif-forecast-missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.forecastquery#AmazonForecastRuntime
Issue-Key: forecastquery.query-whatif-forecast-arn-validation-precedes-lookup|run#7|AssertionError|assert err_malformed["Message"] == "Invalid ARN for what-if-forecast: definitely-not-an-arn", (
Issue-Key: forecastquery.query-whatif-forecast-date-format-validation-rejected|run#9|AssertionError|assert err_bad["Message"] == invalid_arn, (
Issue-Key: forecastquery.query-whatif-forecast-end-date-before-start-date-rejected|run#9|AssertionError|assert err_reversed["Message"] == invalid_arn, (
Issue-Key: forecastquery.query-whatif-forecast-filter-key-invalid-characters-rejected|run#8|AssertionError|assert message_bad == expected_message, (
Issue-Key: forecastquery.query-whatif-forecast-invalid-next-token-rejected|run#9|AssertionError|assert err_bogus["Message"] == invalid_arn, (
Issue-Key: forecastquery.query-whatif-forecast-missing-iam-permission-denied|run#16|AssertionError|assert err_privileged["Message"] == invalid_arn, (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# QueryWhatIfForecast: InvalidInputException message for an unresolvable WhatIfForecastArn diverges from AWS

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryWhatIfForecast

## Description
Cause 1 of 1 — the WhatIfForecastArn rejection message does not follow the AWS rule, on every QueryWhatIfForecast branch.
AWS: Smithy: QueryWhatIfForecast.WhatIfForecastArn is required, targets `LongArn` with @length 0-300 and @pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`; the modeled rejection is `InvalidInputException` (httpError 400, doc 'The value is invalid or is too long.'). Per the recordings, QueryWhatIfForecast never distinguishes a malformed ARN from a well-formed-but-absent one: both yield HTTP 400 InvalidInputException, and the message is composed from the request as `Invalid ARN for what-if-forecast: <the value supplied>`. That rule holds regardless of what else is wrong with the request — a malformed EndDate, a reversed date range, an invalid Filters key, a bogus NextToken, or a caller with zero IAM grants all produce the same code and the same ARN-derived message, because ARN resolution dominates this operation's error path.
Emulator: the code and status are right (400 InvalidInputException) but two different emulator-specific messages are produced instead. For a value that fails the ARN pattern it returns the generic shape-validation text `The value is invalid or is too long. Value '<value>' at 'whatIfForecastArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\d-]+):forecast:.*:.*:.+$`; for a pattern-valid but unresolvable what-if-forecast ARN it returns `The value is invalid: WhatIfForecastArn '<arn>' does not name a what-if-forecast. A what-if-forecast ARN has the form arn:<partition>:forecast:<region>:<account>:what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>.`
Why: a client that surfaces, logs, or matches on the AWS rejection message gets different text for every QueryWhatIfForecast failure, and the emulator's second variant additionally asserts an ARN sub-structure requirement (analysis/forecast name segments) that AWS does not state in this rejection — steering callers to "fix" a correctly shaped ARN.
Fix: QueryWhatIfForecast WhatIfForecastArn validation / InvalidInputException construction in the forecastquery handler — one message rule, `Invalid ARN for what-if-forecast: <supplied value>`, for both the malformed and the unresolvable case, emitted before date, filter, token and authorization handling.
Traces: forecastquery.query-whatif-forecast-arn-validation-precedes-lookup, forecastquery.query-whatif-forecast-date-format-validation-rejected, forecastquery.query-whatif-forecast-end-date-before-start-date-rejected, forecastquery.query-whatif-forecast-filter-key-invalid-characters-rejected, forecastquery.query-whatif-forecast-invalid-next-token-rejected, forecastquery.query-whatif-forecast-missing-iam-permission-denied.

## Evidence
All six QueryWhatIfForecast checks agree on Code=InvalidInputException (HTTP 400) and diverge only on the message rule.

Malformed value — check forecastquery.query-whatif-forecast-arn-validation-precedes-lookup. Expected (AWS rule, example): 'Invalid ARN for what-if-forecast: definitely-not-an-arn'; actual: "The value is invalid or is too long. Value 'definitely-not-an-arn' at 'whatIfForecastArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\\d-]+):forecast:.*:.*:.+$".

Pattern-valid but absent ARN — checks query-whatif-forecast-date-format-validation-rejected, query-whatif-forecast-end-date-before-start-date-rejected, query-whatif-forecast-filter-key-invalid-characters-rejected, query-whatif-forecast-invalid-next-token-rejected, query-whatif-forecast-missing-iam-permission-denied. Expected (AWS rule, example): 'Invalid ARN for what-if-forecast: arn:aws:forecast:us-west-2:000000000000:what-if-forecast/sim2real_fq_24f99d021b53'; actual: "The value is invalid: WhatIfForecastArn 'arn:aws:forecast:us-west-2:000000000000:what-if-forecast/sim2real_fq_24f99d021b53' does not name a what-if-forecast. A what-if-forecast ARN has the form arn:<partition>:forecast:<region>:<account>:what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>."

The divergence is message-rule only in all six; the code, status and dominance order (ARN before dates/filters/token/IAM) already match.

## Steps to Reproduce
1. Point a botocore forecastquery client at the emulator (region us-west-2).
2. query_what_if_forecast(WhatIfForecastArn='definitely-not-an-arn', Filters={'item_id':'x'}) -> 400 InvalidInputException, but the message is the generic pattern-constraint text instead of 'Invalid ARN for what-if-forecast: definitely-not-an-arn'.
3. query_what_if_forecast(WhatIfForecastArn='arn:aws:forecast:us-west-2:<acct>:what-if-forecast/some_name', Filters={'item_id':'x'}) -> 400 InvalidInputException, but the message is "The value is invalid: WhatIfForecastArn '<arn>' does not name a what-if-forecast. ..." instead of 'Invalid ARN for what-if-forecast: <arn>'.
4. Repeat step 3 while also passing EndDate='01-01-2024 00:00:00', or StartDate later than EndDate, or Filters={'item id!':'x'}, or NextToken='bogus', or using credentials of a brand-new IAM user with no policies: every variant returns the same emulator message instead of the ARN-derived AWS message.
