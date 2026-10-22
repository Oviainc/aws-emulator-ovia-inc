Issue-Source: trace-sweep-divergence
Issue-Trace: events-custom-event-bus-config-update
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-custom-event-bus-config-update events-eventbus-create-describe-update-delete events-eventbus-lifecycle events-event-bus-resource-policy-cross-account-permission events-eventbus-logconfig-toggle events-eventbus-multi-isolation events-update-event-bus-description-lifecycle events-update-event-bus-errors events-update-event-bus-dead-letter-config-round-trip events-update-event-bus-full-combined-fields events-update-event-bus-kms-and-log-config
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|DescribeEventBus|2xx->2xx|$.Description
Issue-Key: events|DescribeEventBus|2xx->2xx|$.LogConfig
Issue-Key: events|DescribeEventBus|2xx->2xx|$.Policy
Issue-Key: events|DescribeEventBus|2xx->2xx|$.LogConfig.IncludeDetail
Issue-Key: events|DescribeEventBus|2xx->2xx|$.LogConfig.Level
Issue-Key: events|DescribeEventBus|2xx->2xx|$.DeadLetterConfig
Issue-Key: events|DescribeEventBus|2xx->2xx|$.KmsKeyIdentifier
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# EventBridge event bus configuration is not persisted and DescribeEventBus omits it

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventBus

## Description
The emulator treats an event bus as a name/ARN pair only: every mutable configuration member accepted by UpdateEventBus (and the resource policy installed by PutPermission) is dropped on write, so DescribeEventBus answers 200 with the bus identity and nothing else. Seven distinct configuration members are affected; a fixer must close ALL SEVEN causes below.

Cause 1 of 7 - Description and LogConfig are not stored by UpdateEventBus.
AWS: Smithy UpdateEventBus input accepts Description (EventBusDescription, @length 0..512), LogConfig, DeadLetterConfig and KmsKeyIdentifier, and returns them in UpdateEventBusResponse; DescribeEventBusResponse models Name, Arn, Description, KmsKeyIdentifier, DeadLetterConfig, Policy, LogConfig, CreationTime, LastModifiedTime, ManagedBy. A DescribeEventBus following a successful UpdateEventBus therefore reports the updated configuration.
Emulator: DescribeEventBus returns 200 but $.Description is absent (expected the description just written) and $.LogConfig is absent (expected both IncludeDetail and Level as written).
Why: a client that configures a bus and reads it back to confirm sees an unconfigured bus; config-convergence loops (CDK/Terraform-style read-after-write) re-issue the update forever or report drift.
Fix: UpdateEventBus handler must write these members into the event-bus state record, and the DescribeEventBus serializer must emit them.
Traces: events-custom-event-bus-config-update#task#4; also events-eventbus-create-describe-update-delete#task#4, events-eventbus-lifecycle#task#4.

Cause 2 of 7 - the resource policy is never returned.
AWS: DescribeEventBusResponse models Policy (String) - the bus resource policy, as installed by PutPermission/PutResourcePolicy. The recording shows a 200 whose $.Policy is the IAM policy document containing the statement added for the granted principal.
Emulator: 200 with $.Policy absent.
Why: cross-account grant flows cannot verify a grant landed, and a client that reads-modifies-writes the policy loses existing statements.
Fix: PutPermission must record the statement on the bus state and DescribeEventBus must serialize the assembled policy document.
Traces: events-event-bus-resource-policy-cross-account-permission#task#4.

Cause 3 of 7 - LogConfig stays at defaults after being toggled.
AWS: LogConfig has members IncludeDetail (enum IncludeDetail) and Level (enum Level); UpdateEventBus sets them.
Emulator: DescribeEventBus returns 200 with $.LogConfig.IncludeDetail 'NONE' and $.LogConfig.Level 'OFF' - the hard-coded defaults - instead of the values written (recording example: FULL / TRACE).
Why: logging configuration appears to be silently ignored; a client cannot distinguish 'not supported' from 'not applied'.
Fix: same state/serializer path as cause 1, specifically the LogConfig sub-structure.
Traces: events-eventbus-logconfig-toggle#task#4.

Cause 4 of 7 - Description updates are ignored (stale value retained).
AWS: a second UpdateEventBus replaces Description; DescribeEventBus then reports the new text.
Emulator: DescribeEventBus returns 200 with the ORIGINAL creation-time description (recording example: 'bus A description' where 'bus A updated' was expected), so the create-time value is kept and the update discarded. Note this cluster also covers a DescribeEventBus on a second, isolated bus, so it is not a cross-bus mix-up.
Why: updates report success (200) while having no effect - the worst failure mode for idempotent converge loops.
Fix: UpdateEventBus handler must overwrite, not ignore, an already-set Description.
Traces: events-eventbus-multi-isolation#task#6; also events-update-event-bus-description-lifecycle#task#5, events-update-event-bus-description-lifecycle#task#6, events-update-event-bus-errors#task#4.

Cause 5 of 7 - DeadLetterConfig is not persisted.
AWS: UpdateEventBus accepts DeadLetterConfig{Arn: ResourceArn} and DescribeEventBus returns it; the recording shows $.DeadLetterConfig = {Arn: <sqs queue arn>}.
Emulator: 200 with $.DeadLetterConfig absent, both immediately after the update and after a later round-trip read.
Why: bus-level DLQ configuration is unverifiable, so failure-path tests built on it cannot run.
Fix: state + DescribeEventBus serializer for the DeadLetterConfig member.
Traces: events-update-event-bus-dead-letter-config-round-trip#task#3; also events-update-event-bus-dead-letter-config-round-trip#task#5.

Cause 6 of 7 - a combined update applies none of its members.
AWS: one UpdateEventBus carrying Description + DeadLetterConfig + LogConfig applies all three atomically.
Emulator: 200, then DescribeEventBus shows $.DeadLetterConfig absent, $.LogConfig absent, and $.Description still the creation value (recording example: 'initial' instead of 'combined update').
Why: confirms the drop is in the update/state path, not in one member's serializer - all members of a single request are lost together.
Fix: as causes 1/3/5, exercised through one multi-member request.
Traces: events-update-event-bus-full-combined-fields#task#3.

Cause 7 of 7 - KmsKeyIdentifier is not persisted.
AWS: UpdateEventBus accepts KmsKeyIdentifier (@length 0..2048, pattern ^[a-zA-Z0-9_\-/:]*$) and DescribeEventBus returns it.
Emulator: 200 with $.KmsKeyIdentifier absent (and $.LogConfig absent in the same response), both after the update and after a subsequent read.
Why: a client cannot confirm the bus is CMK-encrypted; encryption-posture checks read as 'unencrypted'.
Fix: state + DescribeEventBus serializer for KmsKeyIdentifier.
Traces: events-update-event-bus-kms-and-log-config#task#6; also events-update-event-bus-kms-and-log-config#task#8.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (events, 22 divergence clusters total).

Cause 1 - events-custom-event-bus-config-update#task#4 DescribeEventBus: expected 200, got 200; $.Description missing (expected 'Updated description for probe bus', got None); $.LogConfig missing (expected {'IncludeDetail': 'FULL', 'Level': 'TRACE'}, got None). 3 steps affected.
Cause 2 - events-event-bus-resource-policy-cross-account-permission#task#4 DescribeEventBus: 200/200; $.Policy missing (expected the policy document with Sid/Effect Allow/Principal AWS/Action events:PutEvents/Resource = the bus ARN, got None).
Cause 3 - events-eventbus-logconfig-toggle#task#4 DescribeEventBus: 200/200; $.LogConfig.IncludeDetail value_mismatch (expected 'FULL', got 'NONE'); $.LogConfig.Level value_mismatch (expected 'TRACE', got 'OFF').
Cause 4 - events-eventbus-multi-isolation#task#6 DescribeEventBus: 200/200; $.Description value_mismatch (expected 'bus A updated', got 'bus A description'). 4 steps affected.
Cause 5 - events-update-event-bus-dead-letter-config-round-trip#task#3 DescribeEventBus: 200/200; $.DeadLetterConfig missing (expected {'Arn': '<sqs dlq arn>'}, got None). 2 steps affected.
Cause 6 - events-update-event-bus-full-combined-fields#task#3 DescribeEventBus: 200/200; $.DeadLetterConfig missing, $.LogConfig missing (expected {'IncludeDetail': 'NONE', 'Level': 'TRACE'}), $.Description value_mismatch (expected 'combined update', got 'initial').
Cause 7 - events-update-event-bus-kms-and-log-config#task#6 DescribeEventBus: 200/200; $.KmsKeyIdentifier missing (expected a kms key ARN, got None); $.LogConfig missing (expected {'IncludeDetail': 'FULL', 'Level': 'INFO'}). 2 steps affected.

Model evidence: Smithy UpdateEventBus input/output = {Name, KmsKeyIdentifier, Description, DeadLetterConfig, LogConfig}; DescribeEventBusResponse = {Name, Arn, Description, KmsKeyIdentifier, DeadLetterConfig, Policy, LogConfig, CreationTime, LastModifiedTime, ManagedBy}. The emulator's responses carry only Name/Arn/CreationTime/LastModifiedTime. ARNs, bus names and timestamps in the expectations above are environment-specific examples; the requirement is that the written member is echoed back.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the wire corpus (replay_wire_corpus) - no arguments.
2. CreateEventBus a custom bus, then UpdateEventBus on it with Description, LogConfig{IncludeDetail,Level}, DeadLetterConfig{Arn} and KmsKeyIdentifier.
3. DescribeEventBus the same bus: the 200 response carries only Name/Arn/CreationTime/LastModifiedTime; Description/LogConfig/DeadLetterConfig/KmsKeyIdentifier are absent or still the creation-time values.
4. For the policy cause: PutPermission granting events:PutEvents to a principal on the bus, then DescribeEventBus - $.Policy is absent.
5. Re-replay the fixtures named in trace_ids plus events-custom-event-bus-config-update#task#4 to confirm all seven causes are closed.
