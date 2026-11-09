Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager.kafka-esm-onfailure-destination-kafka-topic-rules-rejected lambda__secretsmanager.kafka-esm-schema-registry-config-invalid-shape-rejected
Issue-Key: lambda__secretsmanager.kafka-esm-onfailure-destination-kafka-topic-rules-rejected|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Key: lambda__secretsmanager.kafka-esm-schema-registry-config-invalid-shape-rejected|run#23|AssertionError|assert msg == expected, f"expected {expected!r}, got {msg!r}"
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 2 net-new causes: a Kafka ESM accepts a kafka:// on-failure destination, and its schema-registry enum refusal uses the wrong order

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two distinct root causes in CreateEventSourceMapping for a self-managed Kafka source. Both must be closed.

Cause 1 of 2 — a `kafka://<topic>` on-failure destination is accepted.
AWS: Smithy models DestinationConfig.OnFailure.Destination as an ARN-typed string. For a Kafka event source Lambda supports a Kafka topic destination only under the documented rules (the destination must name a topic on the same cluster the mapping consumes, and the request must satisfy the accompanying configuration); a `kafka://<topic>` destination that does not satisfy them is refused at create with InvalidParameterValueException (HTTP 400) and no mapping is left behind.
Emulator: the call SUCCEEDS — HTTP 202 with the mapping created and `DestinationConfig: {'OnFailure': {'Destination': 'kafka://<topic>'}}` persisted, State 'Creating'.
Why: a client's negative configuration case passes and an unusable on-failure destination is persisted; at failure time there is nowhere to write, so failed batches are discarded with no error raised at the point AWS raises it.
Fix: CreateEventSourceMapping's validation of DestinationConfig.OnFailure.Destination for the Kafka source type — apply the Kafka-topic destination rules and refuse with InvalidParameterValueException before persisting.
Traces: lambda__secretsmanager.kafka-esm-onfailure-destination-kafka-topic-rules-rejected.

Cause 2 of 2 — the schema-registry access-config enum refusal lists the enum set in the wrong order.
AWS: an invalid `selfManagedKafkaEventSourceConfig.schemaRegistryConfig.accessConfigs.<i>.member.type` is refused with `1 validation error detected: Value '<value>' at 'selfManagedKafkaEventSourceConfig.schemaRegistryConfig.accessConfigs.<i>.member.type' failed to satisfy constraint: Member must satisfy enum value set: [CLIENT_CERTIFICATE_TLS_AUTH, OAUTHBEARER_AUTH, SERVER_ROOT_CA_CERTIFICATE, BASIC_AUTH]`. The fixed parts are the sentence template and AWS's ordering of the bracketed enum set; the variable parts are the rejected value and the member path (including the list index) taken from the request.
Emulator: emits the same sentence but orders the set `[BASIC_AUTH, CLIENT_CERTIFICATE_TLS_AUTH, SERVER_ROOT_CA_CERTIFICATE, OAUTHBEARER_AUTH]` — the Smithy declaration order rather than AWS's.
Why: a client that asserts on, logs or diffs the validation message gets text AWS never emits, so error-taxonomy checks and operator-facing messages disagree with AWS for an otherwise correct refusal.
Fix: the validation-message serializer for enum constraints on this input — render the enum value set in AWS's fixed order rather than the model's declaration order.
Traces: lambda__secretsmanager.kafka-esm-schema-registry-config-invalid-shape-rejected.

## Evidence
Cause 1 — lambda__secretsmanager.kafka-esm-onfailure-destination-kafka-topic-rules-rejected:
AssertionError: expected a ClientError, the call succeeded: HTTPStatusCode 202, UUID '<uuid>', DestinationConfig {'OnFailure': {'Destination': 'kafka://<topic>'}}, SelfManagedEventSource {'Endpoints': {'KAFKA_BOOTSTRAP_SERVERS': ['<host>:9092']}}, State 'Creating'

Cause 2 — lambda__secretsmanager.kafka-esm-schema-registry-config-invalid-shape-rejected:
AssertionError: expected "...Member must satisfy enum value set: [CLIENT_CERTIFICATE_TLS_AUTH, OAUTHBEARER_AUTH, SERVER_ROOT_CA_CERTIFICATE, BASIC_AUTH]", got "...Member must satisfy enum value set: [BASIC_AUTH, CLIENT_CERTIFICATE_TLS_AUTH, SERVER_ROOT_CA_CERTIFICATE, OAUTHBEARER_AUTH]"
(uuid, topic and host names are per-run values.)

## Steps to Reproduce
Cause 1: CreateEventSourceMapping with SelfManagedEventSource Kafka endpoints, Topics=['<topic>'], a BASIC_AUTH SourceAccessConfiguration, and DestinationConfig={'OnFailure': {'Destination': 'kafka://<dlt topic>'}} — AWS refuses with InvalidParameterValueException; the emulator returns 202.
Cause 2: CreateEventSourceMapping with selfManagedKafkaEventSourceConfig.schemaRegistryConfig.accessConfigs containing an entry whose type is not in the enum; compare the enum value set ordering in the ValidationException message.
