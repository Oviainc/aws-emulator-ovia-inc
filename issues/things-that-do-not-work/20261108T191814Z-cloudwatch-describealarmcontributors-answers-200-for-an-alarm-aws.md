Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.describe-alarm-contributors-nonexistent-alarm-not-found cloudwatch.describe-alarm-contributors-nonexistent-composite-not-found cloudwatch.describe-alarm-contributors-nonexistent-not-found cloudwatch.describe-alarm-contributors-nonexistent-resource cloudwatch.describe-alarm-contributors-rejected-for-non-contributor-alarms cloudwatch.describe-alarm-contributors-invalid-next-token cloudwatch.stop-metric-streams-missing-required-parameter-rejected cloudwatch.get-insight-rule-report-missing-required-param
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.describe-alarm-contributors-nonexistent-alarm-not-found|_client_error#7|AssertionError|raise AssertionError(f"expected rejection, but the call succeeded and returned {result!r}")
Issue-Key: cloudwatch.describe-alarm-contributors-nonexistent-composite-not-found|_client_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.describe-alarm-contributors-nonexistent-not-found|run#9|AssertionError|raise AssertionError("expected ValidationError for a non-contributor-based alarm")
Issue-Key: cloudwatch.describe-alarm-contributors-nonexistent-resource|_client_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.describe-alarm-contributors-rejected-for-non-contributor-alarms|_client_error#17|AssertionError|raise AssertionError(f"expected a ValidationError rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.describe-alarm-contributors-invalid-next-token|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.stop-metric-streams-missing-required-parameter-rejected|run#9|AssertionError|assert code == "ValidationException", (
Issue-Key: cloudwatch.get-insight-rule-report-missing-required-param|run#13|AssertionError|assert message == (
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudwatch: DescribeAlarmContributors answers 200 for an alarm AWS refuses and drops the token from InvalidNextToken; StopMetricStreams and GetInsightRuleReport missing-parameter refusals are emulator-authored (4 causes)

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmContributors

## Description
FOUR net-new behavioral root causes on CloudWatch read/refusal paths. The replay report marks none of the 8 grounding checks ALREADY FILED or OWNED. Nearest open issues, named for the fixer but carrying none of these check keys: 20261108T134822Z-monitoring-cloudwatch-read-and-write-fidelity-describealarmcontributors-skips-its.md (its cause 1 is the same DescribeAlarmContributors alarm-type gate, keyed on wire fixture steps; its cause 4 is the same MissingRequiredParameter-vs-ValidationException substitution on PutMetricStream rather than StopMetricStreams), 20261029T203422Z-monitoring-cloudwatch-contributor-insights-error-paths-use-emulator-authored.md and 20261025T020208Z-cloudwatch-describealarmcontributors-stub.md (the operation now answers; it answers wrongly).

Cause 1 of 4: DescribeAlarmContributors on an alarm that is not a multi-time-series alarm answers 200 with an empty contributor list instead of refusing.
AWS: Smithy: DescribeAlarmContributors takes a required AlarmName and models exactly InvalidNextToken (400, awsQueryError InvalidNextToken) and ResourceNotFoundException (404); the alarm-type precondition is not modeled, so recorded behavior only: for an existing ordinary single-metric alarm — and for a composite alarm over one — AWS refuses with HTTP 400, error code ValidationError, message "Alarm <AlarmName> is not a multi time series alarm". Fixed text: "Alarm " + " is not a multi time series alarm"; the single variable part is the alarm name echoed from the request. A GROUP BY Metrics Insights alarm IS contributor-based and must still succeed, and a name that was never created must still answer 404 ResourceNotFoundException "No alarm for name <name> was found" (the emulator already does this correctly — the defect is only the alarm-type gate).
Emulator: answers HTTP 200 with {"AlarmContributors": []} for the plain metric alarm and for the composite alarm.
Why: this refusal is how a client distinguishes a multi-time-series alarm from a plain one. Getting 200 with an empty list, a caller concludes the plain alarm simply has no contributors right now, so contributor-driven diagnostics and dashboards attribute an empty breakdown to an alarm that has no contributor dimension at all, and the ValidationError branch is never exercised.
Fix: the DescribeAlarmContributors handler's input validation — gate on the alarm's type before projecting contributors, and serialize the ValidationError (400 + code + the message above).
Traces: cloudwatch.describe-alarm-contributors-nonexistent-alarm-not-found, cloudwatch.describe-alarm-contributors-nonexistent-composite-not-found, cloudwatch.describe-alarm-contributors-nonexistent-not-found, cloudwatch.describe-alarm-contributors-nonexistent-resource, cloudwatch.describe-alarm-contributors-rejected-for-non-contributor-alarms.

Cause 2 of 4: the DescribeAlarmContributors InvalidNextToken message drops the offending token.
AWS: Smithy: DescribeAlarmContributors models InvalidNextToken with @httpError 400 and awsQueryError code InvalidNextToken. Recorded behavior: the message is "Invalid NextToken : <token>" — fixed text "Invalid NextToken : " plus the NextToken value taken verbatim from the request.
Emulator: refuses with the right code but the bare message "Invalid NextToken", with no token appended.
Why: when a paging client holds several tokens (or logs the refusal), the echoed token is the only thing identifying which token was rejected; a message-equality or message-parsing client sees a different string than AWS produces.
Fix: the DescribeAlarmContributors pagination-token guard's error message (append the request's NextToken).
Traces: cloudwatch.describe-alarm-contributors-invalid-next-token.

Cause 3 of 4: StopMetricStreams with Names omitted raises MissingRequiredParameterException instead of the service's Coral ValidationException.
AWS: Smithy: StopMetricStreams.Names is @required and the operation does model MissingRequiredParameterException (awsQueryError MissingParameter/400) — but the recorded refusal for a null required member on this service is the Coral input-validation path: HTTP 400 with the ValidationException type (awsQueryError ValidationError) and the message template "1 validation error detected: Value null at '<lower-camel member path>' failed to satisfy constraint: Member must not be null", i.e. 'names' for this request. (The same recorded substitution on the sibling operation PutMetricStream is documented in 20261108T134822Z-monitoring-cloudwatch-read-and-write-fidelity-describealarmcontributors-skips-its.md cause 4; the fixed text is that numbered template, the variable part is the member path taken from the request's missing member.) With Names present the call returns 200.
Emulator: answers HTTP 400 with __type MissingRequiredParameterException and the emulator-authored message "The parameter Names is required."
Why: the status matches but the error code does not, so retry/branch logic keyed on ValidationError never matches, and a parser that reads the "1 validation error detected: ... at '<member>'" template to point the user at the offending member gets prose it cannot parse and loses the lower-camel member path.
Fix: StopMetricStreams' input validation — route missing required members through the same Coral validation serializer the rest of the service uses, not the MissingRequiredParameter path.
Traces: cloudwatch.stop-metric-streams-missing-required-parameter-rejected.

Cause 4 of 4: GetInsightRuleReport with Period omitted uses an emulator-authored MissingParameter message.
AWS: Smithy: GetInsightRuleReport.Period is @required (range min 1) and the operation models MissingRequiredParameterException with awsQueryError MissingParameter/400. Recorded behavior: the message is "MISSING_PERIOD: The Period parameter must be present in a GetContributors request." — all fixed text, nothing taken from the request. With Period present the same call succeeds and returns KeyLabels for the rule's contribution keys.
Emulator: answers the correct code but the message "The parameter Period is required."
Why: the message differs from AWS's wording for this error class, so clients and operators keyed on AWS's text (including the MISSING_PERIOD discriminator AWS prefixes) do not match it.
Fix: GetInsightRuleReport's required-member validation message.
Traces: cloudwatch.get-insight-rule-report-missing-required-param.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; each line is the failing assertion the suite reported. None of the 8 checks is marked ALREADY FILED or OWNED.

Cause 1 — cloudwatch.describe-alarm-contributors-nonexistent-alarm-not-found: AssertionError: expected rejection, but the call succeeded and returned {'AlarmContributors': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}; cloudwatch.describe-alarm-contributors-nonexistent-composite-not-found and cloudwatch.describe-alarm-contributors-nonexistent-resource: same 200 + empty AlarmContributors where a ValidationError was expected (their never-created-name assertions, expecting 404 ResourceNotFoundException "No alarm for name <name> was found", passed first); cloudwatch.describe-alarm-contributors-nonexistent-not-found: AssertionError: expected ValidationError for a non-contributor-based alarm; cloudwatch.describe-alarm-contributors-rejected-for-non-contributor-alarms: AssertionError: expected a ValidationError rejection, but the call SUCCEEDED and returned {'AlarmContributors': [], ...}. Expected in every case: HTTP 400, code ValidationError, message "Alarm <the alarm name sent> is not a multi time series alarm".

Cause 2 — cloudwatch.describe-alarm-contributors-invalid-next-token: AssertionError: expected message 'Invalid NextToken : <the token sent>', got 'Invalid NextToken' (the recorded run used a probe token; the token value is the request's, not part of the template).

Cause 3 — cloudwatch.stop-metric-streams-missing-required-parameter-rejected: AssertionError: omitting Names entirely gave error 'MissingRequiredParameterException', expected ValidationException: {'__type': 'MissingRequiredParameterException', 'message': 'The parameter Names is required.'} — the positive control (Names present -> HTTP 200) passed, so this is isolated to the omitted-member case.

Cause 4 — cloudwatch.get-insight-rule-report-missing-required-param: AssertionError: unexpected MissingParameter message 'The parameter Period is required.' — expected "MISSING_PERIOD: The Period parameter must be present in a GetContributors request." (the error code matched).

Model citations: smithy_query operation=DescribeAlarmContributors,StopMetricStreams,GetInsightRuleReport on services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json (DescribeAlarmContributors: required AlarmName, errors InvalidNextToken/400 + ResourceNotFoundException/404; StopMetricStreams: required Names, errors MissingRequiredParameterException MissingParameter/400, InvalidParameterValueException, InternalServiceFault; GetInsightRuleReport: required RuleName/StartTime/EndTime/Period, errors MissingRequiredParameterException MissingParameter/400, InvalidParameterValueException, ResourceNotFoundException).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite (run_behavioral_suite).
3. Cause 1: replay the five describe-alarm-contributors checks — PutMetricAlarm an ordinary single-metric alarm (and a composite over it), then DescribeAlarmContributors on it: must answer HTTP 400 ValidationError "Alarm <requested alarm name> is not a multi time series alarm"; a GROUP BY Metrics Insights alarm must still succeed and a never-created name must still answer 404 ResourceNotFoundException "No alarm for name <name> was found".
4. Cause 2: replay cloudwatch.describe-alarm-contributors-invalid-next-token — DescribeAlarmContributors with a malformed NextToken must answer InvalidNextToken with message 'Invalid NextToken : <the token sent>'.
5. Cause 3: replay cloudwatch.stop-metric-streams-missing-required-parameter-rejected — a signed StopMetricStreams request with Names absent must answer HTTP 400 ValidationException with the Coral template naming the null 'names' member; with Names present it must still answer 200.
6. Cause 4: replay cloudwatch.get-insight-rule-report-missing-required-param — GetInsightRuleReport without Period must answer MissingParameter "MISSING_PERIOD: The Period parameter must be present in a GetContributors request."; with Period it must return the rule's KeyLabels.
7. Re-replay all 8 check ids listed in trace_ids after the fix; every one must pass.
