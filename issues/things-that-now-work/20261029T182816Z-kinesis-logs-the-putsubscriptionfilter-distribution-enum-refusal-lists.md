Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__logs.subscription-filter-bylogstream-distribution-pins-each-log-stream-to-one-shard
Issue-Key: kinesis__logs.subscription-filter-bylogstream-distribution-pins-each-log-stream-to-one-shard|run#38|AssertionError|assert err.get("Message") == (
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# kinesis__logs: the PutSubscriptionFilter distribution enum refusal lists the enum values in the wrong order (1 cause)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the constraint-violation message for an out-of-vocabulary `distribution` renders the enum value set in a different order than AWS ===
AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) defines com.amazonaws.cloudwatchlogs#Distribution as an enum whose values are, in order, Random then ByLogStream. A PutSubscriptionFilter with distribution outside that set is refused with InvalidParameterException and the standard constraint message "1 validation error detected: Value at 'distribution' failed to satisfy constraint: Member must satisfy enum value set: [Random, ByLogStream]" — the fixed text is the template, and the only variable parts are the member name and the enum value list, which AWS renders in the model's declaration order.
Emulator: the code and template are right but the list is reversed: "... Member must satisfy enum value set: [ByLogStream, Random]". The rest of the refusal is correct (no filter is half-applied).
Why: clients and test suites that compare the validation message byte-for-byte (the usual way these messages are asserted, since the list is part of the fixed template) do not match, and users shown the message see an ordering AWS never emits.
Fix: the enum-validation serializer for the `distribution` member in the PutSubscriptionFilter handler — render the value set in the Smithy declaration order (Random, ByLogStream).
Traces: kinesis__logs.subscription-filter-bylogstream-distribution-pins-each-log-stream-to-one-shard.

## Evidence
Failing check kinesis__logs.subscription-filter-bylogstream-distribution-pins-each-log-stream-to-one-shard (verdict: mismatch). The check's final leg sends PutSubscriptionFilter(..., distribution="ByShard") and asserts the message equals "1 validation error detected: Value at 'distribution' failed to satisfy constraint: Member must satisfy enum value set: [Random, ByLogStream]". Replay: 'AssertionError: PutSubscriptionFilter with distribution=\'ByShard\' returned message "1 validation error detected: Value at \'distribution\' failed to satisfy constraint: Member must satisfy enum value set: [ByLogStream, Random]"'. Error code InvalidParameterException and HTTP 400 are correct; only the value-set ordering diverges.

## Steps to Reproduce
1. logs CreateLogGroup; create a Kinesis stream and a role logs.amazonaws.com can assume.
2. logs PutSubscriptionFilter logGroupName=<group> filterName=<name> filterPattern="" destinationArn=<stream ARN> roleArn=<role> distribution="ByShard".
   Observed: InvalidParameterException "1 validation error detected: Value at 'distribution' failed to satisfy constraint: Member must satisfy enum value set: [ByLogStream, Random]".
   Expected: the same text with the value set rendered as [Random, ByLogStream].
