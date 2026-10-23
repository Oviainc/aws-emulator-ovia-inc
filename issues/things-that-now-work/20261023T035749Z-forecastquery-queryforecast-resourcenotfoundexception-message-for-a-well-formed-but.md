Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: forecastquery.query-forecast-arn-validation-precedes-lookup
Issue-Shape-Id: com.amazonaws.forecastquery#AmazonForecastRuntime
Issue-Key: forecastquery.query-forecast-arn-validation-precedes-lookup|run#12|AssertionError|assert err_missing["Message"] == f"No resource found {nonexistent_arn}", (
Issue-Commit: a25176d5b2a976d6230e754c8e419e3d957ef7d7

# QueryForecast ResourceNotFoundException message for a well-formed but absent ForecastArn does not follow the AWS request-derived rule

> _Note: this issue was identified against the code base at commit `a25176d5b2a976d6230e754c8e419e3d957ef7d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryForecast

## Description
One root cause, in the QueryForecast not-found error path of the forecastquery emulator.

Cause 1 of 1 — the ResourceNotFoundException returned when ForecastArn is syntactically valid but resolves to no forecast carries an emulator-specific message instead of the AWS message rule.

AWS: Smithy (services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json): QueryForecast.ForecastArn is @required, targets Arn with @length 0-256 and @pattern ^arn:([a-z\d-]+):forecast:.*:.*:.+$; QueryForecast's modeled errors include ResourceNotFoundException with httpError 404. The model does not state the message text, so the message rule is recorded behavior only: across every recorded branch that passes input validation and reaches resource resolution, real AWS returns HTTP 404 with error code ResourceNotFoundException and a message composed from the request as `No resource found <the ForecastArn supplied>`. The rule is request-derived (it echoes the submitted ARN verbatim and adds no other prose), which is why it is a requirement and not just one recording's text.

Emulator: the status (404) and the error code (ResourceNotFoundException) are correct, but the message is of the form `We can't find that resource. No forecast with ARN <arn> exists.` — the shape's documentation sentence plus emulator-authored prose around the ARN.

Why: a client or test suite that matches on the AWS message shape, or that parses the offending ARN back out of the message to report or retry on it, gets no match and either mis-reports the failure or treats a correctly-classified not-found as an unknown error, even though the code is right.

Fix: the QueryForecast handler's ResourceNotFoundException construction / error serialization for the resource-resolution path — the message must be built as `No resource found ` + the ForecastArn from the request, with no additional text.

Relation to already-filed work: the open issue 20261022T200802Z-forecastquery-queryforecast-shape-level-input-validation-skipped-startdate-filters.md describes this same not-found message divergence as its Cause 3, but its recorded replay keys for that cause are other checks; this check (forecastquery.query-forecast-arn-validation-precedes-lookup) now fails on the not-found message assertion and is not carried by that issue's keys, so it is filed here. Its malformed-ARN branch (InvalidInputException code and message `Invalid ARN for forecast: definitely-not-an-arn`) now passes, so only the not-found message remains.

Traces: forecastquery.query-forecast-arn-validation-precedes-lookup.

## Evidence
Check forecastquery.query-forecast-arn-validation-precedes-lookup (mismatch):
- Branch 1 (passes): query_forecast(ForecastArn='definitely-not-an-arn', Filters={'item_id': 'sim2real-item'}) -> 400 InvalidInputException, message 'Invalid ARN for forecast: definitely-not-an-arn'. Matches AWS.
- Branch 2 (fails): query_forecast(ForecastArn='arn:aws:forecast:us-west-2:<account>:forecast/<absent-name>', Filters={'item_id': 'sim2real-item'}).
  Expected (AWS rule, example values): Code='ResourceNotFoundException', HTTP 404, Message='No resource found arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_288054bad79e'.
  Actual: {'Code': 'ResourceNotFoundException', 'Message': "We can't find that resource. No forecast with ARN arn:aws:forecast:us-west-2:000000000000:forecast/sim2real_fq_288054bad79e exists."}
  Assertion: AssertionError: unexpected ResourceNotFoundException message for absent ARN (check line 45).
Status and error code agree; only the request-derived message text diverges.

## Steps to Reproduce
1. Point a botocore forecastquery client at the emulator (region us-west-2).
2. acct = sts.get_caller_identity()['Account'].
3. Call fq.query_forecast(ForecastArn=f'arn:aws:forecast:us-west-2:{acct}:forecast/does_not_exist_abc', Filters={'item_id': 'sim2real-item'}).
4. Observe the ClientError: Code is ResourceNotFoundException (correct, HTTP 404) but Message is "We can't find that resource. No forecast with ARN <arn> exists." instead of AWS's 'No resource found <arn>'.
5. Optional control: the same call with ForecastArn='definitely-not-an-arn' correctly yields InvalidInputException with 'Invalid ARN for forecast: definitely-not-an-arn', showing ARN-shape validation still precedes lookup.
