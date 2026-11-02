Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-create-rejected-for-unsupported-kafka-version kafka.configuration-partial-invalid-kafka-versions-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-create-rejected-for-unsupported-kafka-version|run#6|AssertionError|assert message.startswith("Unsupported KafkaVersion [0.0.1-does-not-exist]. Valid values: [") and (
Issue-Key: kafka.configuration-partial-invalid-kafka-versions-rejected|run#14|AssertionError|assert message.startswith(f"Unsupported KafkaVersion [{_BOGUS_VERSION}]. Valid values: [") and (
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# CreateConfiguration's unsupported-KafkaVersions refusal Message is the emulator's own prose and omits the valid-value list (2 causes)

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
TWO distinct causes, both in CreateConfiguration's KafkaVersions validation path. The emulator picks the right HTTP status (400) and the right Error.Code (BadRequestException), but writes its own sentence into Error.Message instead of the fixed text Amazon MSK uses for an unsupported Kafka version, and it drops the valid-value list that is the actionable part of that refusal. Both causes must be closed.

NOTE on a neighbouring filing: the open issue 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md describes this same message family as its Cause 7, but its replay key for that cause is the check kafka.configuration-unsupported-kafka-version-rejected only. The two checks below are NOT carried by it (the suite reports them as unowned, failing at different assertions in different check files), so they are filed here and must be re-replayed in their own right; fixing Cause 7 there is expected to close these too, but nothing currently gates on them.

Cause 1 of 2 — a KafkaVersions list whose single entry is not a version MSK offers.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations, success 200) takes KafkaVersions (list of string, jsonName 'kafkaVersions') and models BadRequestException (httpError 400). The model does not state the message, so: recorded behavior only — MSK refuses 400 / Error.Code 'BadRequestException' with Error.Message of the form "Unsupported KafkaVersion [<the rejected version>]. Valid values: [<the versions the service offers, comma-separated, in the service's own order>]". FIXED text: "Unsupported KafkaVersion [", then "]. Valid values: [", then the closing "]". VARIABLE parts: the rejected version string, taken verbatim from the request's KafkaVersions entry, and the supported-version list, taken from service state (the same catalogue ListKafkaVersions serves). The list contents of any one run are an example of that variable part, never part of the template. The refusal is also an atomic no-op: the requested Name must not appear in ListConfigurations afterwards.
Emulator: 400 / 'BadRequestException' with "Amazon MSK supports none of these Apache Kafka versions: '<version>'." — a different sentence, with no "Unsupported KafkaVersion [...]" prefix and no valid-value list at all.
Why: a client that classifies or displays MSK's refusal text sees a string AWS never emits, and one that reads the supported versions out of the error text (the only place this refusal offers them) gets nothing and cannot retry with a valid version.
Fix: the KafkaVersions validation and its error construction in the CreateConfiguration handler (services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs) — emit MSK's template and render the valid-value list from the same state ListKafkaVersions reads.
Traces: kafka.configuration-create-rejected-for-unsupported-kafka-version.

Cause 2 of 2 — a PARTIALLY valid KafkaVersions list is reported as wholly unsupported.
AWS: recorded behavior only — when KafkaVersions mixes a supported version with an unsupported one (e.g. a valid version plus a bogus one), MSK refuses the whole request 400 / 'BadRequestException' and the Message names the OFFENDING entry alone, in the same template as Cause 1: "Unsupported KafkaVersion [<the offending version>]. Valid values: [<the service's version list>]". The valid entry is not named and the request is not partially applied: the Name must be absent from ListConfigurations, and in particular must not have been created with the unsupported entry silently filtered out. A pre-existing configuration and its revision stream are unaffected (UpdateConfiguration on the control still advances to revision 2).
Emulator: the same self-composed sentence as Cause 1, "Amazon MSK supports none of these Apache Kafka versions: '<version>'." — besides the wrong template it is factually wrong for this input class: it asserts that NONE of the submitted versions are supported when one of them is, so the plural phrasing misreports which entry the caller must change.
Why: the caller is told its whole version list is bad when exactly one entry is, so it cannot narrow the fix; and the message still omits the valid-value list.
Fix: same component as Cause 1 — the KafkaVersions check in the CreateConfiguration handler must identify the first unsupported entry and name only it, rather than summarising the list.
Traces: kafka.configuration-partial-invalid-kafka-versions-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 48/215 passed. Both failures are [mismatch] and both reach the Message assertion, i.e. the HTTP status (400) and Error.Code ('BadRequestException') were already correct — only Error.Message diverges.

Cause 1 — kafka.configuration-create-rejected-for-unsupported-kafka-version (mismatch):
  kafka.create_configuration(Name=<new>, KafkaVersions=['0.0.1-does-not-exist'], ServerProperties=b'auto.create.topics.enable=true\n')
  expected Error.Message to start with "Unsupported KafkaVersion [0.0.1-does-not-exist]. Valid values: [" and to end with "]"
  actual   AssertionError: Amazon MSK supports none of these Apache Kafka versions: '0.0.1-does-not-exist'.
  (check_kafka_configuration_create_rejected_for_unsupported_kafka_version.py line 31, assertion `message.startswith("Unsupported KafkaVersion [0.0.1-does-not-exist]. Valid values: [")`)

Cause 2 — kafka.configuration-partial-invalid-kafka-versions-rejected (mismatch):
  control: create_configuration(Name=<ok>, KafkaVersions=['3.9.x'], ServerProperties=b'auto.create.topics.enable=true\n') -> ACTIVE, KafkaVersions readback ['3.9.x'] (passes)
  then     create_configuration(Name=<mixed>, KafkaVersions=['3.9.x', '9.9.9'], ServerProperties=b'auto.create.topics.enable=true\n')
  expected code 'BadRequestException', HTTP 400 (both observed OK) and Error.Message starting with "Unsupported KafkaVersion [9.9.9]. Valid values: [" and ending with "]"
  actual   AssertionError: Amazon MSK supports none of these Apache Kafka versions: '9.9.9'.
  (check_kafka_configuration_partial_invalid_kafka_versions_rejected.py line 107)

Smithy cited: kafka#CreateConfiguration — POST /v1/configurations, code 200; input members Name (@required), ServerProperties (@required, blob), KafkaVersions (list of string); errors include BadRequestException with httpError 400. The model carries no message text, hence "recorded behavior only" above.

## Steps to Reproduce
1. Boot the emulator.
2. kafka.list_kafka_versions() -> note the catalogue (3.9.x must be ACTIVE); this is the list the refusal message must render.
3. Cause 1: kafka.create_configuration(Name='cfg-badver', KafkaVersions=['0.0.1-does-not-exist'], ServerProperties=b'auto.create.topics.enable=true\n') -> expect 400 BadRequestException with Error.Message 'Unsupported KafkaVersion [0.0.1-does-not-exist]. Valid values: [<versions from step 2>]'. Observe the emulator's "Amazon MSK supports none of these Apache Kafka versions: '0.0.1-does-not-exist'." Then kafka.list_configurations() -> 'cfg-badver' must be absent.
4. Cause 2: kafka.create_configuration(Name='cfg-ok', KafkaVersions=['3.9.x'], ServerProperties=b'auto.create.topics.enable=true\n') and wait for ACTIVE; then kafka.create_configuration(Name='cfg-mixed', KafkaVersions=['3.9.x','9.9.9'], ServerProperties=b'auto.create.topics.enable=true\n') -> expect HTTP 400 / BadRequestException with Error.Message 'Unsupported KafkaVersion [9.9.9]. Valid values: [<versions from step 2>]' — naming ONLY the unsupported entry. Observe the "supports none of these" sentence instead.
5. kafka.list_configurations() -> 'cfg-mixed' must be absent, and must not exist with a filtered KafkaVersions ['3.9.x'].
6. kafka.describe_configuration(Arn=<cfg-ok>) -> still ACTIVE with KafkaVersions ['3.9.x']; kafka.update_configuration(Arn=<cfg-ok>, ServerProperties=b'auto.create.topics.enable=true\nnum.partitions=2\n') -> LatestRevision.Revision == 2.
7. Re-replay kafka.configuration-create-rejected-for-unsupported-kafka-version and kafka.configuration-partial-invalid-kafka-versions-rejected.
