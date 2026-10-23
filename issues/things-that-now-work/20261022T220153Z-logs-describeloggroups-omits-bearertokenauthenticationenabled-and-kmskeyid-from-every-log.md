Issue-Source: trace-sweep-divergence
Issue-Trace: logs-bearer-token-auth-and-aggregate-summaries
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-bearer-token-auth-and-aggregate-summaries logs-create-log-group-with-class-and-tags logs-describe-queries-pagination logs-insights-query-multi-log-group logs-insights-query-single-log-group logs-log-group-lifecycle-and-fields logs-log-group-lifecycle-and-tags logs-log-anomaly-detector-lifecycle logs-logfields-and-loggroupfields-and-querygroups logspc-kms-key-associate-disassociate
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[arn="arn
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*]
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].bearerTokenAuthenticationEnabled
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].kmsKeyId
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# DescribeLogGroups omits bearerTokenAuthenticationEnabled and kmsKeyId from every log group

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
The LogGroup structure the emulator serializes in DescribeLogGroups is missing members the recorded service always populates. bearerTokenAuthenticationEnabled is absent from every log group the emulator describes -- whether the flag was turned on or left at its default -- and kmsKeyId is absent after a KMS key has been associated with the log group. Status is 200 on both sides; the defect is in the response projection and in the state the mutating operations persist. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4 -- bearerTokenAuthenticationEnabled is not reported as true after it has been enabled.
AWS: Smithy -- LogGroup.bearerTokenAuthenticationEnabled targets BearerTokenAuthenticationEnabled (boolean), documented as "Indicates whether bearer token authentication is enabled for this log group. When enabled, bearer token authentication is allowed on operations until it is explicitly disabled." DescribeLogGroups returns logGroups: LogGroups. Recorded: for a log group with bearer-token auth enabled, the described LogGroup carries bearerTokenAuthenticationEnabled = true.
Emulator: the member is absent from the described log group (null), so the enabled state is unobservable.
Why: a client cannot read back whether bearer-token auth is on. Any control loop that enables the flag and then verifies it (or that decides between SigV4 and bearer-token auth from the described state) sees an unconfigured log group and either loops forever or re-enables on every pass. This cluster spans ten fixture steps, the widest DescribeLogGroups failure in the corpus.
Fix: the DescribeLogGroups response serializer plus the log-group state that the bearer-token enable path writes -- the flag must be persisted and projected onto LogGroup.
Traces: logs-bearer-token-auth-and-aggregate-summaries#task#3 (also the same fixture's task#5, logs-create-log-group-with-class-and-tags#task#2, logs-describe-queries-pagination#task#7, logs-insights-query-multi-log-group#task#8, logs-insights-query-single-log-group#task#5, logs-log-group-lifecycle-and-fields#task#2, logs-log-group-lifecycle-and-tags#task#2 / #task#7 / #task#9).

Cause 2 of 4 -- bearerTokenAuthenticationEnabled is not reported as false by default.
AWS: as cause 1. Recorded: a freshly created log group is described with bearerTokenAuthenticationEnabled = false -- the member is present and explicitly false, not omitted.
Emulator: the member is absent.
Why: the member being absent rather than false means a client cannot distinguish "disabled" from "this emulator/region does not support the feature", and a strict deserializer that expects the documented boolean sees a null where a boolean is modeled.
Fix: DescribeLogGroups response serializer -- emit the default false.
Traces: logs-logfields-and-loggroupfields-and-querygroups#task#2 (also logspc-kms-key-associate-disassociate#task#8).

Cause 3 of 4 -- the full default LogGroup projection is incomplete.
AWS: as cause 1. Recorded: the LogGroup for a newly created log group carries arn, bearerTokenAuthenticationEnabled (false), creationTime, deletionProtectionEnabled (false), logGroupArn, logGroupClass, logGroupName, metricFilterCount and storedBytes.
Emulator: the same projection minus bearerTokenAuthenticationEnabled -- every other member is present and correctly shaped (the differing creationTime value is environment-dependent and not a requirement).
Why: as cause 2; recorded here on the whole-object comparison so a fixer can see the projection is otherwise complete and only the one member is missing.
Fix: DescribeLogGroups response serializer.
Traces: logs-log-anomaly-detector-lifecycle#setup#2.

Cause 4 of 4 -- kmsKeyId is not reported after a KMS key is associated with the log group.
AWS: Smithy -- LogGroup.kmsKeyId targets KmsKeyId @length 0..256, documented as "The Amazon Resource Name (ARN) of the KMS key to use when encrypting log data." Recorded: after the KMS key is associated, DescribeLogGroups reports kmsKeyId as the key ARN; bearerTokenAuthenticationEnabled is false on the same object.
Emulator: both kmsKeyId and bearerTokenAuthenticationEnabled are absent -- the key association is not reflected in the described log group at all.
Why: encryption configuration is write-only in the emulator. A client cannot confirm a log group is CMK-encrypted, cannot detect drift, and cannot tell an associate from a disassociate -- so the whole associate/disassociate lifecycle is unverifiable.
Fix: the AssociateKmsKey / DisassociateKmsKey state write plus the DescribeLogGroups response serializer -- persist kmsKeyId on the log group and project it.
Traces: logspc-kms-key-associate-disassociate#task#6.

## Evidence
All four steps return HTTP 200 on both sides; the divergence is missing response members.
- Cause 1 (logs-bearer-token-auth-and-aggregate-summaries#task#3, 10 affected steps): $.logGroups[arn="arn:aws:logs:<region>:<account>:log-group:<name>:*"].bearerTokenAuthenticationEnabled expected True, got None.
- Cause 2 (logs-logfields-and-loggroupfields-and-querygroups#task#2, 2 affected steps): $.logGroups[0].bearerTokenAuthenticationEnabled expected False, got None.
- Cause 3 (logs-log-anomaly-detector-lifecycle#setup#2): $.logGroups[0] expected {arn, bearerTokenAuthenticationEnabled: False, creationTime, deletionProtectionEnabled: False, logGroupArn, logGroupClass: 'STANDARD', logGroupName, metricFilterCount: 0, storedBytes: 0}; actual carries every member EXCEPT bearerTokenAuthenticationEnabled.
- Cause 4 (logspc-kms-key-associate-disassociate#task#6): $.logGroups[0].kmsKeyId expected the associated KMS key ARN, got None; $.logGroups[0].bearerTokenAuthenticationEnabled expected False, got None.
Smithy citations: LogGroup members bearerTokenAuthenticationEnabled -> BearerTokenAuthenticationEnabled (boolean) and kmsKeyId -> KmsKeyId @length 0..256; DescribeLogGroups output logGroups -> LogGroups, nextToken.
The concrete ARNs, account id, region and creationTime values in the recordings are environment-dependent and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-bearer-token-auth-and-aggregate-summaries step task#3 and read logGroups[].bearerTokenAuthenticationEnabled -- the member is absent where the recording has true.
3. Replay logs-logfields-and-loggroupfields-and-querygroups#task#2 -- the member is absent where the recording has false.
4. Replay logs-log-anomaly-detector-lifecycle#setup#2 and compare the whole logGroups[0] object -- only bearerTokenAuthenticationEnabled is missing.
5. Replay logspc-kms-key-associate-disassociate#task#6 -- kmsKeyId is absent after the key association, as well as bearerTokenAuthenticationEnabled.
