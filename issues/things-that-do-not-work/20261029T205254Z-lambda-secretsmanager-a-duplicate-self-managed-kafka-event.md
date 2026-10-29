Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager.self-managed-kafka-esm-duplicate-for-the-same-topic-rejected
Issue-Key: lambda__secretsmanager.self-managed-kafka-esm-duplicate-for-the-same-topic-rejected|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Commit: d5ace36132e41d92d7f91429edc3b2c2caee7d54

# lambda__secretsmanager: a duplicate self-managed Kafka event source mapping for the same function, bootstrap server and topic is accepted (202) instead of refused with ResourceConflictException (1 cause)

> _Note: this issue was identified against the code base at commit `d5ace36132e41d92d7f91429edc3b2c2caee7d54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: CreateEventSourceMapping does not detect that an identical self-managed Kafka source is already mapped to the same function, so a second mapping is minted.
AWS: Smithy: CreateEventSourceMapping models ResourceConflictException (client error, HTTP 409) alongside InvalidParameterValueException/ResourceNotFoundException, and its success code is 202. The source identity of a self-managed Kafka mapping is SelfManagedEventSource.Endpoints.KAFKA_BOOTSTRAP_SERVERS + Topics + the target function (see https://docs.aws.amazon.com/lambda/latest/dg/with-kafka.html); a second create for that same triple is refused with ResourceConflictException and the message template 'An event source mapping with event source ("<bootstrap server from SelfManagedEventSource.Endpoints.KAFKA_BOOTSTRAP_SERVERS>"), function ("<the function's ARN>"), topics ("<topic from Topics>") already exists. Please update or delete the existing mapping with UUID <the existing mapping's UUID from state>'. The fixed text is everything outside those four slots: the endpoint, the topic and the function come from the request, the UUID comes from the mapping already in state. The function keeps exactly one mapping; after DeleteEventSourceMapping of that mapping the identical request is accepted again with a new UUID.
Emulator: the duplicate request answers HTTP 202 with a second mapping - UUID 97057430-ef52-4097-a677-33e794cea1ab, State 'Creating', the same Topics ['sim2real-topic'], the same SelfManagedEventSource endpoints and the same SourceAccessConfigurations BASIC_AUTH secret - so the function now lists two mappings for one Kafka source and no conflict is ever reported.
Why: a client relying on Lambda's conflict detection (re-running a deployment, retrying a create after a timeout, or racing two deployers) ends up with duplicate pollers for one topic instead of a 409 it can treat as "already configured"; records would be delivered twice in a real system, and the "update or delete the existing mapping with UUID ..." hint a caller needs to converge is never produced.
Fix: the duplicate-source conflict check in the lambda crate's CreateEventSourceMapping handler (compute the self-managed-Kafka source identity from SelfManagedEventSource endpoints + Topics + resolved function ARN, look it up in the event-source-mapping state, and refuse with ResourceConflictException carrying the message template above).
Traces: lambda__secretsmanager.self-managed-kafka-esm-duplicate-for-the-same-topic-rejected.

## Evidence
Check lambda__secretsmanager.self-managed-kafka-esm-duplicate-for-the-same-topic-rejected.
Setup: a Secrets Manager secret holding BASIC_AUTH credentials, an execution role granting secretsmanager:GetSecretValue on it, a python3.12 function, and CreateEventSourceMapping(FunctionName=<fn>, Topics=['sim2real-topic'], StartingPosition='LATEST', SelfManagedEventSource={'Endpoints': {'KAFKA_BOOTSTRAP_SERVERS': ['sim2real.invalid:9092']}}, SourceAccessConfigurations=[{'Type':'BASIC_AUTH','URI':<secret arn>}]) - accepted, UUID U1.
Expected (AWS): the identical second CreateEventSourceMapping -> 409 ResourceConflictException, message 'An event source mapping with event source ("sim2real.invalid:9092"), function ("<function arn>"), topics ("sim2real-topic") already exists. Please update or delete the existing mapping with UUID <U1>'; ListEventSourceMappings for the function still returns exactly [U1].
Actual (emulator): HTTPStatusCode 202 and a second mapping: {'UUID': '97057430-ef52-4097-a677-33e794cea1ab', 'State': 'Creating', 'StateTransitionReason': 'USER_INITIATED', 'Topics': ['sim2real-topic'], 'SelfManagedEventSource': {'Endpoints': {'KAFKA_BOOTSTRAP_SERVERS': ['sim2real.invalid:9092']}}, 'EventSourceMappingArn': 'arn:aws:lambda:us-west-2:000000000000:event-source-mapping:97057430-...'}. The check's assertion "expected a ClientError, the call succeeded" fired at check_lambda__secretsmanager_self_managed_kafka_esm_duplicate_for_the_same_topic_rejected.py line 58.

## Steps to Reproduce
1. CreateSecret with a JSON {username,password} body; create an execution role allowing secretsmanager:GetSecretValue on that ARN; CreateFunction (python3.12, trivial handler).
2. CreateEventSourceMapping(FunctionName=<fn>, Topics=['sim2real-topic'], StartingPosition='LATEST', SelfManagedEventSource={'Endpoints': {'KAFKA_BOOTSTRAP_SERVERS': ['sim2real.invalid:9092']}}, SourceAccessConfigurations=[{'Type':'BASIC_AUTH','URI':<secret arn>}]) and note the returned UUID.
3. Issue the byte-identical CreateEventSourceMapping again. Expect HTTP 409 ResourceConflictException whose message names the bootstrap server, the function ARN, the topic and the first mapping's UUID; the emulator answers 202 with a new UUID.
4. ListEventSourceMappings(FunctionName=<fn>) must still show exactly one mapping; then DeleteEventSourceMapping it and replay the identical create, which must be accepted with a new UUID.
