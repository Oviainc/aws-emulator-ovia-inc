Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis
Issue-Key: kms__logs|CreateStream|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# kms__logs: the kinesis CreateStream stub blocks the only check that grades whether a CMK-encrypted log group delivers plaintext to a subscription-filter destination (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a subscription filter on a CMK-encrypted log group cannot be exercised because its Kinesis destination cannot be created ===
AWS: CloudWatch Logs decrypts a CMK-encrypted log group's events on the service side before fan-out, so the records a subscription filter delivers to a Kinesis stream are plaintext (gzip-framed CWL envelopes), not ciphertext — the group's CMK protects data at rest in CWL and does not propagate to the destination. Verifying that requires a Kinesis stream as the subscription-filter destination.
Emulator: kinesis CreateStream answers NotImplemented ('operation CreateStream is scaffolded but not implemented in this emulator'), so the destination never exists and the check never reaches the logs assertion.
Why: the interaction between log-group encryption and subscription-filter fan-out is unverified; a client cannot tell whether the emulator would deliver plaintext (correct) or refuse/encrypt. Note: the kinesis service is still in this project's PENDING bootstrap set, so its bootstrap may resolve the blocking call — the logs-side assertion still needs a re-replay afterwards, which is why this is recorded against the kms__logs group rather than left implicit.
Fix: kinesis CreateStream (services/service-com-amazonaws-kinesis-kinesis-20131202), then re-replay this check to grade the CWL fan-out path for a CMK-encrypted group.
Traces: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis.

## Evidence
kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator. Expected a created stream, then a PutSubscriptionFilter on the CMK-encrypted log group, then plaintext CWL records read off the stream.

## Steps to Reproduce
kms CreateKey with a policy granting logs.<region>.amazonaws.com; CreateLogGroup with that kmsKeyId; kinesis CreateStream(StreamName=..., ShardCount=1) — NotImplemented. (Then PutSubscriptionFilter(destinationArn=<stream ARN>, roleArn=...), PutLogEvents, and GetRecords to inspect the delivered records.)
