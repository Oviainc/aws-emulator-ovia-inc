Issue-Source: trace-sweep-divergence
Issue-Trace: logspc-subscription-filter-lambda-destination-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logspc-subscription-filter-lambda-destination-lifecycle logs-subscription-filter-lifecycle logs-subscription-filter-to-destination-pipeline
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutSubscriptionFilter|4xx->2xx|$
Issue-Key: logs|PutSubscriptionFilter|4xx->2xx|status
Issue-Key: logs|DescribeSubscriptionFilters|2xx->2xx|$.subscriptionFilters[*].distribution
Issue-Key: logs|DescribeSubscriptionFilters|2xx->2xx|$.subscriptionFilters[*]
Issue-Key: logs|PutSubscriptionFilter|4xx->4xx|$.message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# PutSubscriptionFilter skips filterPattern and destinationArn validation, and drops distribution

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
The emulator's subscription-filter surface accepts a malformed filter pattern the recorded service refuses, then serves the bogus filter back on read, and it loses the filter's distribution setting. A fourth cause shows PutSubscriptionFilter refusing a malformed destinationArn with an emulator-authored message instead of the service's request-derived parse error. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4 -- PutSubscriptionFilter accepts an unbalanced filter pattern.
AWS: Smithy -- PutSubscriptionFilter.filterPattern targets FilterPattern @length 0..1024 and is required; the operation models InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException and ServiceUnavailableException. Recorded: a filter pattern that opens with `{` but never closes is refused with 400 and __type InvalidParameterException; the message states the brace-balance rule, so the rule that produces it is known (a pattern beginning with `{` must end with `}`).
Emulator: 200 with an empty body -- the malformed pattern is stored.
Why: the whole point of server-side filter-pattern validation is to fail fast at configuration time. The emulator lets a broken subscription be created, so a client's negative test passes and the misconfiguration surfaces only as silently undelivered log events.
Fix: PutSubscriptionFilter input validation -- parse filterPattern and refuse an unbalanced JSON-style pattern with the modeled InvalidParameterException at 400.
Traces: logspc-subscription-filter-lambda-destination-lifecycle#task#8.

Cause 2 of 4 -- DescribeSubscriptionFilters omits the filter's distribution.
AWS: Smithy -- SubscriptionFilter.distribution targets the Distribution enum, and PutSubscriptionFilter.distribution accepts it on write. Recorded: the described filter carries distribution = 'ByLogStream'.
Emulator: the member is absent from the described filter (null).
Why: the distribution setting controls how log events are spread across destination shards. A client cannot read back what it configured, cannot detect drift, and a read-modify-write cycle silently resets the setting to the destination's default.
Fix: the PutSubscriptionFilter state write plus the DescribeSubscriptionFilters response serializer -- persist distribution and project it onto SubscriptionFilter.
Traces: logspc-subscription-filter-lambda-destination-lifecycle#task#7.

Cause 3 of 4 -- DescribeSubscriptionFilters serves back the filter that should never have been created.
AWS: Smithy -- DescribeSubscriptionFilters output subscriptionFilters: SubscriptionFilters. Recorded: the filter list is empty, because the malformed-pattern write in cause 1 was refused.
Emulator: returns the stored filter, carrying the truncated/unbalanced filterPattern, the lambda destinationArn, applyOnTransformedLogs false and a creationTime.
Why: the read-side proof of cause 1 and the client-visible damage: the emulator advertises an active subscription whose pattern the real service rejected, so a consumer believes delivery is configured when on AWS it is not. A fixer must re-replay this step after fixing cause 1 to confirm the filter is absent.
Fix: DescribeSubscriptionFilters reads subscription-filter state; follows from cause 1.
Traces: logspc-subscription-filter-lambda-destination-lifecycle#task#10.

Cause 4 of 4 -- the malformed-destinationArn refusal is not the service's request-derived parse error.
AWS: Smithy -- PutSubscriptionFilter.destinationArn targets DestinationArn @length min 1, required. Recorded: a destinationArn that is not an ARN is refused with 400 under the service's validation-error template, naming the unparsable value it was given ("1 validation error detected: Unable to parse destinationArn <value>"). The message is built from the request, so that rule -- a validation-error-detected message naming destinationArn and the value it could not parse -- is the requirement.
Emulator: same 400, but with an emulator-authored message that enumerates the destination services it will accept instead of reporting an ARN parse failure for the submitted value.
Why: the refusal does not follow the service's validation-error template, so tooling that parses the refusal to report which member failed (and on what value) extracts nothing; and the emulator's message implies a valid-ARN-wrong-service failure where the service reported a parse failure, pointing a caller at the wrong fix.
Fix: PutSubscriptionFilter input validation -- emit the service's request-derived validation-error message for an unparsable destinationArn.
Traces: logs-subscription-filter-lifecycle#task#3 (also logs-subscription-filter-to-destination-pipeline#task#6).

## Evidence
Cause 1 (logspc-subscription-filter-lambda-destination-lifecycle#task#8): status expected 400, got 200; body expected {'__type': 'InvalidParameterException', 'message': "If a filter pattern starts with '{' it must end with '}'"}, got {} (empty body). The 400 status, the InvalidParameterException code and the brace-balance rule are the requirement.
Cause 2 (same fixture task#7, 200 on both sides): $.subscriptionFilters[0].distribution expected 'ByLogStream', got None.
Cause 3 (same fixture task#10, 200 on both sides): $.subscriptionFilters[0] expected None, got a populated SubscriptionFilter {filterName, logGroupName, filterPattern: '{ $.level = ', destinationArn: <lambda arn>, applyOnTransformedLogs: False, creationTime} -- note the stored filterPattern is the unbalanced one from cause 1.
Cause 4 (logs-subscription-filter-lifecycle#task#3, 2 affected steps, 400 on both sides): $.message expected '1 validation error detected: Unable to parse destinationArn <submitted value>', got 'destinationArn must be the ARN of a kinesis, firehose, lambda, logs resource'.
Smithy citations: PutSubscriptionFilter.filterPattern -> FilterPattern @length 0..1024 (required), .destinationArn -> DestinationArn @length min 1 (required), .distribution -> Distribution enum; errors [InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException]; SubscriptionFilter.distribution -> Distribution; DescribeSubscriptionFilters output subscriptionFilters -> SubscriptionFilters.
The lambda ARN, filter name and creationTime in the recordings are environment-dependent.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logspc-subscription-filter-lambda-destination-lifecycle step task#8: PutSubscriptionFilter with a filterPattern that starts with '{' and never closes. The emulator answers 200 with an empty body; the recording answers 400 InvalidParameterException.
3. Replay the same fixture's task#7 (DescribeSubscriptionFilters) and read subscriptionFilters[0].distribution -- absent where the recording has 'ByLogStream'.
4. Replay the same fixture's task#10 (DescribeSubscriptionFilters) -- the emulator returns the filter created in step 2; the recording returns an empty list.
5. Replay logs-subscription-filter-lifecycle#task#3 (PutSubscriptionFilter with destinationArn='not-a-valid-arn') and read $.message -- the emulator's text is its own, not the service's 'Unable to parse destinationArn <value>' validation-error form.
