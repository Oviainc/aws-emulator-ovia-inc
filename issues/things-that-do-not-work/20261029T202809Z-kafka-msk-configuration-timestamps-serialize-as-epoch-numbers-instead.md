Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-configuration-create-list-paginate
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-configuration-create-list-paginate kafka-configuration-error-paths kafka-configuration-lifecycle kafka-configuration-revision-lifecycle kafka-tagging-error-paths kafka-configuration-optional-fields-roundtrip
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateConfiguration|2xx->2xx|$.creationTime
Issue-Key: kafka|CreateConfiguration|2xx->2xx|$.latestRevision.creationTime
Issue-Key: kafka|DescribeConfiguration|2xx->2xx|$.creationTime
Issue-Key: kafka|DescribeConfiguration|2xx->2xx|$.latestRevision.creationTime
Issue-Key: kafka|ListConfigurations|2xx->2xx|$.configurations[arn="arn
Issue-Key: kafka|DescribeConfigurationRevision|2xx->2xx|$.creationTime
Issue-Key: kafka|UpdateConfiguration|2xx->2xx|$.latestRevision.creationTime
Issue-Key: kafka|ListConfigurationRevisions|2xx->2xx|$.revisions[*]
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK configuration timestamps serialize as epoch numbers instead of ISO-8601 date-times (6 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
SIX distinct root causes of one kind: every MSK configuration read and write path serializes `creationTime` as a bare JSON number (a fractional Unix epoch) where the model requires an ISO-8601 date-time string. The HTTP status and every other member are correct, so the response deserializes into a type error rather than an error response - an SDK that maps the member to a date type fails to parse, and a hand-rolled client silently gets a number. Six separate serializer sites emit it, so all six must be closed.

AWS (common to all six): Smithy kafka#Configuration member CreationTime targets `__timestampIso8601` with jsonName 'creationTime' and timestampFormat 'date-time', and is @required; kafka#ConfigurationRevision member CreationTime is the same shape, jsonName 'creationTime', timestampFormat 'date-time', @required. The wire form is therefore an RFC3339/ISO-8601 date-time STRING (e.g. "2026-10-29T20:18:48.610Z"). The instant itself is state-derived and differs per run; only the TYPE and format are the requirement.

Emulator (common to all six): a JSON number such as 1793305128.6100218 in place of the date-time string, at both the top-level `creationTime` and the nested `latestRevision.creationTime`.

Why (common to all six): this is a type divergence, not a value one. A generated SDK client raises a deserialization error on an otherwise-200 response, so every configuration workflow breaks at the first read; a client that tolerates it computes with seconds-since-epoch where it expects a timestamp, so comparisons and TTL arithmetic are wrong by nine orders of magnitude.

Fix (common to all six): the service's timestamp serializer for `__timestampIso8601` must honour the member's timestampFormat 'date-time' instead of falling back to the epoch-seconds default. Fixing the shared serializer under services/service-com-amazonaws-kafka-kafka/src/ (and the shared Configuration/ConfigurationRevision projection helper the handlers call) closes all six; patching one operation's response leaves the others diverging. The same defect was fixed for the sibling service in 20261023T071142Z-kafkaconnect-serializes-worker-configuration-creationtime-as-an-epoch.md, so the correct form is already established in this repo - that issue is resolved and does not cover kafka.

Cause 1 of 6: CreateConfiguration response - both `creationTime` and `latestRevision.creationTime` are numbers. Fix: ops/create_configuration.rs response projection.
Traces: kafka-configuration-create-list-paginate#setup#1.

Cause 2 of 6: DescribeConfiguration response - both members. Fix: ops/describe_configuration.rs.
Traces: kafka-configuration-create-list-paginate#task#5.

Cause 3 of 6: ListConfigurations response - `configurations[].creationTime` and `configurations[].latestRevision.creationTime`. Fix: ops/list_configurations.rs.
Traces: kafka-configuration-lifecycle#task#2.

Cause 4 of 6: DescribeConfigurationRevision response - `creationTime`. Fix: ops/describe_configuration_revision.rs.
Traces: kafka-configuration-revision-lifecycle#task#3.

Cause 5 of 6: UpdateConfiguration response - `latestRevision.creationTime`. Fix: ops/update_configuration.rs.
Traces: kafka-configuration-revision-lifecycle#task#4.

Cause 6 of 6: ListConfigurationRevisions response - `revisions[].creationTime`. The whole revision element therefore fails to match; `revision` and `description` are correct. Fix: ops/list_configuration_revisions.rs.
Traces: kafka-configuration-revision-lifecycle#task#6.

## Evidence
Corpus: simparity-wire-aws-v1. All six clusters verdict=mismatch with expected_status == actual_status == 200; the expected value is recorded as '<shaped-not-asserted>', i.e. the SHAPE of the member is asserted (a date-time string) while the instant is not.

Cause 1 - kafka-configuration-create-list-paginate#setup#1, CreateConfiguration:
  $.creationTime: expected a date-time string, got 1793305128.6100218
  $.latestRevision.creationTime: expected a date-time string, got 1793305128.6100218
  (cluster also affects kafka-configuration-create-list-paginate#setup#2, kafka-configuration-error-paths#setup#1, kafka-configuration-lifecycle#task#1, kafka-configuration-revision-lifecycle#task#1, kafka-tagging-error-paths#setup#1 - 6 steps)

Cause 2 - kafka-configuration-create-list-paginate#task#5, DescribeConfiguration:
  $.creationTime and $.latestRevision.creationTime: expected date-time strings, got 1793305128.6246119
  (cluster also affects kafka-configuration-optional-fields-roundtrip#task#4, kafka-configuration-revision-lifecycle#task#2)

Cause 3 - kafka-configuration-lifecycle#task#2, ListConfigurations:
  $.configurations[arn="..."].creationTime and .latestRevision.creationTime: expected date-time strings, got 1793305129.9776258
  (cluster also affects kafka-configuration-optional-fields-roundtrip#task#3)

Cause 4 - kafka-configuration-revision-lifecycle#task#3, DescribeConfigurationRevision:
  $.creationTime: expected a date-time string, got 1793305131.2200658
  (cluster also affects kafka-configuration-revision-lifecycle#task#5)

Cause 5 - kafka-configuration-revision-lifecycle#task#4, UpdateConfiguration:
  $.latestRevision.creationTime: expected a date-time string, got 1793305131.247557

Cause 6 - kafka-configuration-revision-lifecycle#task#6, ListConfigurationRevisions:
  $.revisions[0]: expected {'revision': 1, 'creationTime': <date-time string>, 'description': 'sim2real probe base configuration'}, got {'creationTime': 1793305131.2200658, 'description': 'sim2real probe base configuration', 'revision': 1}
  $.revisions[1]: same shape, got creationTime 1793305131.247557

Smithy cited: kafka#Configuration.CreationTime -> target __timestampIso8601, jsonName 'creationTime', timestampFormat 'date-time', required; kafka#ConfigurationRevision.CreationTime -> same, required. (The epoch values above are one run's instants and are not part of the requirement; the requirement is the date-time string type.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay kafka-configuration-create-list-paginate#setup#1 - CreateConfiguration must answer 200 with creationTime and latestRevision.creationTime as ISO-8601 date-time STRINGS, not JSON numbers.
3. Cause 2: kafka-configuration-create-list-paginate#task#5 - DescribeConfiguration, same assertion on both members.
4. Cause 3: kafka-configuration-lifecycle#task#2 - ListConfigurations, same assertion on every configurations[] element and its latestRevision.
5. Cause 4: kafka-configuration-revision-lifecycle#task#3 - DescribeConfigurationRevision creationTime.
6. Cause 5: kafka-configuration-revision-lifecycle#task#4 - UpdateConfiguration latestRevision.creationTime.
7. Cause 6: kafka-configuration-revision-lifecycle#task#6 - ListConfigurationRevisions, every revisions[] element's creationTime.
8. Confirm with a generated SDK client that each response deserializes into its date type without error.
9. Re-replay all six fixture#step bindings.
