Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-unsupported-kafka-version-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-unsupported-kafka-version-rejected|run#11|AssertionError|assert message == f"Unsupported KafkaVersion [{_BOGUS_VERSION}]. Valid values: [{_VALID_VALUES}]", (
Issue-Commit: 7195bbe1a56e23a345d06755b702cd8074478e3d

# CreateConfiguration's unsupported-KafkaVersion refusal renders the wrong valid-value list: catalog order instead of MSK's order, and an extra version MSK does not offer (2 causes)

> _Note: this issue was identified against the code base at commit `7195bbe1a56e23a345d06755b702cd8074478e3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
TWO causes, both in the valid-value list that CreateConfiguration's unsupported-KafkaVersion refusal renders. The HTTP status (400), the Error.Code (BadRequestException) and the fixed text of the message ("Unsupported KafkaVersion [" ... "]. Valid values: [" ... "]") are now correct — what diverges is the second variable part of that message, the comma-separated list of versions the caller may retry with. The emulator renders crate::state::KAFKA_VERSION_CATALOG verbatim (see unsupported_kafka_version_message in services/service-com-amazonaws-kafka-kafka/src/state.rs, which joins every catalog entry in catalog order), so both the ORDER and the MEMBERSHIP of the list are the emulator's own rather than MSK's. Both causes must be closed.

NOTE on neighbouring open issues, neither of which covers this: 20261102T083811Z-kafka-createconfiguration-s-unsupported-kafkaversions-refusal-message-is-the.md covers the message TEMPLATE (the emulator's own prose / missing list) for two other checks — the template is now emitted, this issue is about the list the template interpolates. 20261030T072234Z-kafka-msk-version-catalog-is-stale-listkafkaversions-and-getcompatiblekafkaversions.md covers the catalog as served by ListKafkaVersions / GetCompatibleKafkaVersions (a wire finding); it names neither this message nor the '3.8.link' entry, and fixing it does not by itself fix the ORDER this message must use.

Cause 1 of 2 — the valid-value list is emitted in the emulator's ascending catalog order, not the order MSK names the versions in.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations, success 200; input KafkaVersions -> __listOf__string, jsonName 'kafkaVersions'; error BadRequestException with httpError 400). The model constrains neither the versions nor their order, so: recorded behavior only — MSK answers 400 / Error.Code 'BadRequestException' with Error.Message "Unsupported KafkaVersion [<rejected version>]. Valid values: [<the versions MSK offers, comma-separated, in MSK's own fixed order for this message>]". FIXED text: "Unsupported KafkaVersion [", "]. Valid values: [", and the closing "]". VARIABLE parts: the rejected version string, taken verbatim from the request's KafkaVersions entry, and the version list, taken from service state in the order the service publishes it. That order is NOT sorted by version number: the recording shows the newest KRaft releases and the specialised lines first and the oldest release last (as recorded: 4.2.x.kraft, 4.1.x.kraft, 2.8.2.tiered, 4.0.x.kraft, 2.4.1.1, 3.3.2, 3.5.1, 3.6.0, 3.1.1, 3.2.0, 3.3.1, 3.4.0, 2.1.0, 3.7.x, 3.8.x, 2.6.2, 2.7.1, 2.8.0, 2.6.3, 2.7.2, 2.8.1, 2.5.1, 2.6.0, 2.6.1, 2.7.0, 3.6.0.1, 3.9.x, 2.3.1, 2.4.1, 3.7.x.kraft, 2.2.1, 3.8.x.kraft, 3.9.x.kraft, 1.1.1 — an example of that variable part, which is a service-fixed sequence, not a per-run value).
Emulator: the same request gets the list joined in KAFKA_VERSION_CATALOG order, i.e. ascending by version (1.1.1, 2.1.0, 2.2.1, 2.3.1, ... 4.0.x.kraft, 4.1.x.kraft, 4.2.x.kraft), so every position of the list differs from MSK's.
Why: a client that parses the valid-value list out of this refusal (the only place the refusal offers the retryable versions) and picks, say, the first entry as "the newest version MSK offers" picks the OLDEST one against the emulator and retries with a deprecated version; any client that compares or snapshots MSK's refusal text sees a string AWS never emits.
Fix: the valid-value rendering in crate::state::unsupported_kafka_version_message (services/service-com-amazonaws-kafka-kafka/src/state.rs), and/or the order the shared version catalog it reads stores its entries in — the message must render the service's published order rather than assuming the catalog's ascending order is it.
Traces: kafka.configuration-unsupported-kafka-version-rejected.

Cause 2 of 2 — the list names a version MSK does not offer ('3.8.link').
AWS: recorded behavior only — the set in this refusal is exactly the set of Apache Kafka versions MSK offers for a configuration (the same catalogue ListKafkaVersions serves); it contains no entry outside it. In the recording the list carries 34 entries and no '3.8.link'.
Emulator: the message's list carries 35 entries, including '3.8.link' (KAFKA_VERSION_CATALOG holds KafkaVersionEntry { version: "3.8.link", status: DEPRECATED }), a version that is not part of MSK's published catalogue. Because the same catalogue backs the acceptance gate (kafka_version_index), CreateConfiguration also ACCEPTS KafkaVersions=['3.8.link'], which AWS refuses.
Why: the refusal advertises a version as retryable that real MSK rejects, so a client that retries with the advertised value succeeds against the emulator and then fails against AWS — and a configuration minted for a non-existent version cannot be applied to any cluster.
Fix: the shared Apache Kafka version catalogue in services/service-com-amazonaws-kafka-kafka/src/state.rs (KAFKA_VERSION_CATALOG) that both the acceptance gate and this message read — it must hold exactly the versions MSK publishes.
Traces: kafka.configuration-unsupported-kafka-version-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 (kafka 56/215 passed). Failing check [mismatch] kafka.configuration-unsupported-kafka-version-rejected, at check_kafka_configuration_unsupported_kafka_version_rejected.py line 69 — the assertions on the HTTP status and on Error.Code 'BadRequestException' (line 61/68) passed, so only Error.Message diverged.

Request replayed (after a positive control create_configuration with KafkaVersions=['3.9.x'] that succeeded and reached ACTIVE):
  kafka.create_configuration(Name=<unique>, KafkaVersions=['9.9.9'], ServerProperties=b'num.partitions=1\n')

expected Error.Message:
  'Unsupported KafkaVersion [9.9.9]. Valid values: [4.2.x.kraft, 4.1.x.kraft, 2.8.2.tiered, 4.0.x.kraft, 2.4.1.1, 3.3.2, 3.5.1, 3.6.0, 3.1.1, 3.2.0, 3.3.1, 3.4.0, 2.1.0, 3.7.x, 3.8.x, 2.6.2, 2.7.1, 2.8.0, 2.6.3, 2.7.2, 2.8.1, 2.5.1, 2.6.0, 2.6.1, 2.7.0, 3.6.0.1, 3.9.x, 2.3.1, 2.4.1, 3.7.x.kraft, 2.2.1, 3.8.x.kraft, 3.9.x.kraft, 1.1.1]'  (34 entries, MSK's own order)
actual   AssertionError: unexpected message: 'Unsupported KafkaVersion [9.9.9]. Valid values: [1.1.1, 2.1.0, 2.2.1, 2.3.1, 2.4.1, 2.4.1.1, 2.5.1, 2.6.0, 2.6.1, 2.6.2, 2.6.3, 2.7.0, 2.7.1, 2.7.2, 2.8.0, 2.8.1, 2.8.2.tiered, 3.1.1, 3.2.0, 3.3.1, 3.3.2, 3.4.0, 3.5.1, 3.6.0, 3.6.0.1, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.8.link, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft, 4.2.x.kraft]'  (35 entries, ascending catalog order, extra '3.8.link')

The divergence is exactly: (1) ordering — every list position differs; (2) membership — the emulator's list contains '3.8.link', which the recorded list does not.

Code: services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs lines 251-264 build the refusal via crate::state::unsupported_kafka_version_message(unknown); services/service-com-amazonaws-kafka-kafka/src/state.rs lines 894-903 join KAFKA_VERSION_CATALOG entries in catalog order, and line 747 is the '3.8.link' entry.

Smithy cited: kafka#CreateConfiguration — POST /v1/configurations, code 200; KafkaVersions (list of __string, jsonName 'kafkaVersions'); errors include BadRequestException (httpError 400). No member carries an enum of Kafka versions and nothing in the model fixes the message text, hence "recorded behavior only" for the list's content and order.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers; bootstrap-kafka is resolved).
2. kafka.list_kafka_versions() -> confirm 3.9.x is ACTIVE (this is the catalogue the refusal must render).
3. Positive control: kafka.create_configuration(Name='cfg-ok', KafkaVersions=['3.9.x'], ServerProperties=b'num.partitions=1\n') -> 200, State ACTIVE.
4. kafka.create_configuration(Name='cfg-badver', KafkaVersions=['9.9.9'], ServerProperties=b'num.partitions=1\n') -> HTTP 400 / Error.Code 'BadRequestException' (both already correct). Inspect Error.Message: it must be 'Unsupported KafkaVersion [9.9.9]. Valid values: [<the versions MSK offers, in MSK's own published order for this message>]'. Observe instead the catalogue joined in ascending version order, starting '1.1.1, 2.1.0, ...' (Cause 1) and containing '3.8.link' (Cause 2).
5. Cause 2 cross-check: kafka.create_configuration(Name='cfg-link', KafkaVersions=['3.8.link'], ServerProperties=b'num.partitions=1\n') -> the emulator ACCEPTS it (200/ACTIVE) because the same catalogue backs the gate; AWS refuses it with the same unsupported-KafkaVersion refusal.
6. kafka.list_configurations() -> 'cfg-badver' must be absent and 'cfg-ok' present (both already hold).
7. Re-replay kafka.configuration-unsupported-kafka-version-rejected.
