Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.create-with-invalid-input-should-fail
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.create-with-invalid-input-should-fail|_assert_rejected#10|AssertionError|assert code is not None, (
Issue-Commit: 4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12

# CreateConfiguration accepts a Name outside MSK's allowed character set (spaces and punctuation) and mints an ACTIVE configuration

> _Note: this issue was identified against the code base at commit `4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
ONE root cause: CreateConfiguration's Name is never checked against MSK's allowed character set, so a name containing spaces and punctuation is accepted and a configuration is minted.

This is NOT covered by the open issue 20261029T175822Z-kafka-createconfiguration-performs-no-serverproperties-or-name-validation-four.md: that filing's four causes are the ServerProperties java-properties parse, the unknown-key allow-list, the reserved broker-property denylist, and the Name LENGTH bound (1..=128). A character-set gate is a different rule - the offending name here is 29 characters, well inside the length bound, so every one of those four causes can be closed and this check still fails.

Cause 1 of 1 - the Name character set is not enforced.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations, success 200) declares Name as @required targeting __string with NO pattern or length trait, and models BadRequestException (httpError 400) among its errors - so the rule is recorded behavior only, not modeled. Recorded behavior: MSK restricts a configuration name to its documented character set (alphanumerics plus '-', '.' and '_'); a name containing an embedded space or a '!' is refused with HTTP 400 and an Error.Code in MSK's input-rejection family (the check pins the rejection class - AccessDeniedException / BadRequestException / ForbiddenException / NotFoundException / UnauthorizedException / ValidationException / ConflictException - and BadRequestException is the one this operation models), and no configuration is created: the name never appears in ListConfigurations. The check establishes a positive control first - a legally-named CreateConfiguration on the same credentials succeeds - so the refusal is provably about the name and not a blanket denial.
Emulator: HTTP 200. The illegal name is accepted and a configuration is minted (Arn, CreationTime, Name echoed back verbatim, State 'ACTIVE', LatestRevision.Revision 1), so the check's rejection probe records no error code at all.
Why: fail-open. CreateConfiguration is the boundary that exists to catch a bad configuration name, and a caller is handed a configuration ARN whose name real MSK would never have issued. A deploy pipeline validating names against the emulator ships a name that fails in production, and the emulator's own store now holds an identifier outside the service's namespace, which later ARN parsing and name-keyed lookups are not written for.
Fix: validate Name against the allowed character set in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs, in the same pre-state pass as the length bound that issue already asks for, refusing with BadRequestException (400) and writing nothing on refusal.
Traces: kafka.create-with-invalid-input-should-fail.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 43/215 passed.

kafka.create-with-invalid-input-should-fail (mismatch):
  positive control: create_configuration(Name='sim2real-behav-good-config-<rand>', ServerProperties=b'auto.create.topics.enable=true\n') -> 200, Arn returned (control passed, so the account is not denying everything)
  probe: create_configuration(Name='sim2real behav illegal name!', ServerProperties=b'auto.create.topics.enable=true\n')
  expected a rejection whose Error.Code is one of ['AccessDeniedException','BadRequestException','ConflictException','ForbiddenException','NotFoundException','UnauthorizedException','ValidationException']
  actual   AssertionError: CreateConfiguration(illegal name) SUCCEEDED on an invalid target - overly permissive; real AWS rejects this
  (raised at check_kafka_create_with_invalid_input_should_fail.py:192 from _assert_rejected, i.e. the probe returned no error code at all)

The check's two earlier cases PASSED - CreateCluster with nonexistent ClientSubnets and CreateClusterV2 with neither Provisioned nor Serverless are both rejected - which isolates the failure to the configuration-name gate. The three cases after it (CreateTopic / CreateVpcConnection / CreateReplicator parented on a nonexistent cluster, plus the trailing assertion that no phantom cluster leaked into ListClustersV2) were never reached and are not claimed here.

Smithy cited: kafka#CreateConfiguration POST /v1/configurations, success 200; input Name (__string, @required, jsonName 'name') and ServerProperties (__blob, @required) plus optional Description/KafkaVersions; errors BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), ServiceUnavailableException(503), TooManyRequestsException(429), UnauthorizedException(401). Name carries no @pattern and no @length in the model, so the character-set rule is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Positive control: kafka.create_configuration(Name='cfg-legal-name', ServerProperties=b'auto.create.topics.enable=true\n') -> expect 200. This proves the credentials are not being denied wholesale.
3. kafka.create_configuration(Name='sim2real behav illegal name!', ServerProperties=b'auto.create.topics.enable=true\n') -> expect HTTP 400 BadRequestException. Observe HTTP 200 with State 'ACTIVE', LatestRevision.Revision 1 and Name echoed back verbatim.
4. kafka.list_configurations() -> the illegal name must be absent, and must not appear in any sanitised/rewritten form.
5. Also assert the gate is a character-set gate and not the length gate: a 29-character legal name must still be accepted, and a name differing only by an embedded space must be refused.
6. Re-replay kafka.create-with-invalid-input-should-fail.
