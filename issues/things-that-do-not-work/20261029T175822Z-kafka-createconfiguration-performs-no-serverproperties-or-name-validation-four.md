Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-invalid-server-properties-rejected kafka.configuration-invalid-server-property-rejected kafka.configuration-property-value-type-accepted kafka.configuration-reserved-broker-property-rejected kafka.configuration-name-over-length-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-invalid-server-properties-rejected|_error_code#4|AssertionError|raise AssertionError(f"expected CreateConfiguration to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: kafka.configuration-invalid-server-property-rejected|_fail#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp}")
Issue-Key: kafka.configuration-property-value-type-accepted|_fail#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp}")
Issue-Key: kafka.configuration-reserved-broker-property-rejected|_fail#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp}")
Issue-Key: kafka.configuration-name-over-length-rejected|_fail#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp}")
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# CreateConfiguration performs no ServerProperties or Name validation: four inputs MSK refuses are accepted and minted (4 causes)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
FOUR distinct root causes, all in CreateConfiguration's input validation: the handler stores whatever it is given. Each is an over-permissiveness divergence - the emulator answers HTTP 200 and mints an ACTIVE configuration with revision 1 for input real MSK refuses with HTTP 400 BadRequestException. All four must be closed.

NOTE: the open issue 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md names kafka.configuration-invalid-server-properties-rejected, but its replay key for it is the 501-era `assert code == "BadRequestException"` line. CreateConfiguration now answers 200 instead, so the check fails at a different point (an unexpected SUCCESS) and Cause 1 is not covered by that filing.

Cause 1 of 4 - ServerProperties that is not a valid Java-properties document is accepted.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations) requires ServerProperties (@required, blob) and models BadRequestException (httpError 400). Recorded behavior: a ServerProperties body whose line carries no '=' at all (e.g. b'this line has no equals sign and is not valid java properties syntax') is refused 400 / Error.Code 'BadRequestException', and the configuration is NOT created - its Name never appears in ListConfigurations.
Emulator: HTTP 200. It returns Arn, CreationTime, Name, State 'ACTIVE' and LatestRevision.Revision 1, i.e. it mints the configuration.
Why: the caller is handed a handle to a configuration that cannot be attached to a cluster; a deploy pipeline that relies on CreateConfiguration to validate its properties file ships a broken file and fails much later, at cluster-attach time, with no pointer back to the bad input.
Fix: parse ServerProperties as a Java properties document in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs before any state is written, and write nothing on refusal.
Traces: kafka.configuration-invalid-server-properties-rejected.

Cause 2 of 4 - a server-property KEY that is not a Kafka broker property is accepted (and the per-property refusal envelope is missing).
AWS: CreateConfiguration models BadRequestException (400). Recorded behavior: MSK validates the ServerProperties KEY against its allow-list of real Kafka broker properties. A syntactically valid line whose key is unknown (e.g. b'totally.not.a.kafka.property=1\n') is refused 400 / 'BadRequestException', the configuration is absent from ListConfigurations, and the Message is MSK's per-property JSON envelope - a JSON array of objects with members "name", "value" and "errorMessages", whose single error message reads "Key '<key>' is not supported by at least one Apache Kafka version. Key checked against versions: [<the version list MSK checked against>]". Fixed text: the envelope shape and the two sentences; variable: the offending key and its value, both taken verbatim from the request, and the version list, taken from service state. Note the matching recorded NON-behavior: MSK does NOT validate the VALUE - num.partitions=not-a-number, num.partitions=-5, auto.create.topics.enable=maybe and compression.type=bogus all return 200 and store the value byte-identical in revision 1, so the fix must gate on the key only and must not add a type checker.
Emulator: HTTP 200 for the unknown key, configuration minted ACTIVE/revision 1.
Why: an emulator that accepts arbitrary keys lets a typo'd broker property through the boundary that exists to catch it; and because the refusal must name the offending key and value, a caller cannot even be told which line to fix.
Fix: an allow-list check of each parsed KEY in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs, plus the per-property JSON message envelope. Do not validate values.
Traces: kafka.configuration-invalid-server-property-rejected, kafka.configuration-property-value-type-accepted.

Cause 3 of 4 - an MSK-reserved broker property is accepted.
AWS: CreateConfiguration models BadRequestException (400). Recorded behavior: a REAL Kafka property that MSK reserves for itself - broker.id is the measured case (b'broker.id=7\n') - is refused 400 / 'BadRequestException' with the SAME per-property envelope as Cause 2, naming broker.id and its value. This is a separate gate from Cause 2: a 'looks like a Kafka property' heuristic passes Cause 2 and still has to refuse this.
Emulator: HTTP 200, configuration minted ACTIVE/revision 1.
Why: letting a customer set broker.id is letting them corrupt broker identity; an emulator used to validate configurations before production rollout green-lights it.
Fix: a reserved-key denylist, evaluated alongside the allow-list, in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs.
Traces: kafka.configuration-reserved-broker-property-rejected.

Cause 4 of 4 - the Name length bound is not enforced at either end.
AWS: CreateConfiguration requires Name (@required). Recorded behavior: the configuration-name length limit is 128 characters; a 128-character name is created and read back byte-identical, while BOTH 129 characters and the empty string are refused 400 / Error.Code 'BadRequestException' with Message exactly "The resource name contains too many or too few characters" - all fixed text, the offending name is not echoed. One constraint, both ends, one message. The refused name must be absent from ListConfigurations and must NOT have been silently truncated to 128 and created.
Emulator: HTTP 200 for a 129-character name; it mints the configuration and echoes Name back at full 129-character length.
Why: the caller's own name-length guard is the only thing standing between it and a name MSK will refuse in production; and a store that accepts unbounded names is a state-growth hazard.
Fix: length validation (1..=128) on Name in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs, emitting the single length message for both the over-long and the empty case.
Traces: kafka.configuration-name-over-length-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed. All four causes fail as an unexpected SUCCESS (the check's helper raises rather than reporting a code mismatch, precisely so an accepted-where-AWS-refuses cannot be laundered into a code diff).

Cause 1 - kafka.configuration-invalid-server-properties-rejected (mismatch):
  create_configuration(Name='sim2real-kafka-cfg-bad-<rand>', ServerProperties=b'this line has no equals sign and is not valid java properties syntax')
  expected 400 BadRequestException and the name absent from ListConfigurations
  actual   AssertionError: expected CreateConfiguration to be rejected, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:kafka:us-west-2:000000000000:configuration/sim2real-kafka-cfg-bad-<rand>/<uuid>-1', 'LatestRevision': {..., 'Revision': 1}, 'State': 'ACTIVE'}

Cause 2 - kafka.configuration-invalid-server-property-rejected (mismatch):
  create_configuration(Name=<new>, KafkaVersions=['3.9.x'], ServerProperties=b'totally.not.a.kafka.property=1\n')
  expected 400 BadRequestException with the per-property envelope naming the key
  actual   AssertionError: expected the call to be rejected, but it succeeded: {'HTTPStatusCode': 200, 'Arn': '...configuration/sim2real-behav-config-badprop-<rand>/<uuid>-2', 'State': 'ACTIVE'}
kafka.configuration-property-value-type-accepted (mismatch) - the same unknown-KEY assertion, from the check that also pins the matching NON-behavior (bad VALUES must be accepted):
  actual   AssertionError: expected the call to be rejected, but it succeeded: {'HTTPStatusCode': 200, 'Arn': '...configuration/sim2real-behav-config-badkey-<rand>/<uuid>-6', 'State': 'ACTIVE'}

Cause 3 - kafka.configuration-reserved-broker-property-rejected (mismatch):
  create_configuration(Name=<new>, KafkaVersions=['3.6.0'], ServerProperties=b'broker.id=7\n')
  expected 400 BadRequestException, Message = the per-property envelope naming broker.id / 7
  actual   AssertionError: expected the call to be rejected, but it succeeded: {'HTTPStatusCode': 200, 'Arn': '...configuration/sim2real-behav-config-reserved-<rand>/<uuid>-2', 'State': 'ACTIVE'}

Cause 4 - kafka.configuration-name-over-length-rejected (mismatch):
  128-char name created OK (control), then create_configuration(Name=<129 chars>, KafkaVersions=['3.9.x'], ServerProperties=b'auto.create.topics.enable=true\n')
  expected 400 BadRequestException, Message 'The resource name contains too many or too few characters'; same for Name=''
  actual   AssertionError: expected the call to be rejected, but it succeeded: {'HTTPStatusCode': 200, 'Name': 'sim2real-behav-cfgname-<rand>aaaa...abc' (129 chars), 'State': 'ACTIVE'}

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.create_configuration(Name='cfg-bad', ServerProperties=b'this line has no equals sign and is not valid java properties syntax') -> expect 400 BadRequestException; then kafka.list_configurations() -> expect 'cfg-bad' absent. Observe 200 with State ACTIVE.
3. Cause 2: kafka.create_configuration(Name='cfg-badkey', KafkaVersions=['3.9.x'], ServerProperties=b'totally.not.a.kafka.property=1\n') -> expect 400 BadRequestException whose Message is the JSON array [ { "name": "totally.not.a.kafka.property", "value": "1", "errorMessages": [ "Key '...' is not supported by at least one Apache Kafka version. Key checked against versions: [...]" ] } ]. Observe 200.
4. Cause 2 (non-behavior to preserve): kafka.create_configuration with ServerProperties=b'num.partitions=not-a-number\n', then b'num.partitions=-5\n', b'auto.create.topics.enable=maybe\n', b'compression.type=bogus\n' -> each must still return 200 and store the value byte-identical in revision 1.
5. Cause 3: kafka.create_configuration(Name='cfg-reserved', KafkaVersions=['3.6.0'], ServerProperties=b'broker.id=7\n') -> expect 400 BadRequestException with the per-property envelope naming broker.id. Observe 200.
6. Cause 4: create a 128-character name (expect 200, Name round-trips byte-identical), then the same name with one extra character (129) -> expect 400 BadRequestException, Message 'The resource name contains too many or too few characters'; then Name='' -> expect the identical code and Message. Finally list_configurations() -> neither the 129-char name nor its 128-char truncation may be present.
7. Re-replay every check id in trace_ids.
