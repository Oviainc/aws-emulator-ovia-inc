Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.delete-nonexistent-resources-should-fail
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.delete-nonexistent-resources-should-fail|run#17|AssertionError|assert describe_gone == _MISSING_CONFIGURATION, (
Issue-Commit: df75a05586985baf68c72ff4d4d2cf208170da4a

# kafka DescribeConfiguration answers 404 NotFoundException with an invented message where MSK answers 400 BadRequestException "Configuration ARN does not exist." (1 cause)

> _Note: this issue was identified against the code base at commit `df75a05586985baf68c72ff4d4d2cf208170da4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfiguration

## Description
Cause 1 of 1: DescribeConfiguration on an MSK configuration ARN this region holds no configuration for (here: one that was created and then deleted, so the delete is observable only through this read) is refused with the wrong error class, HTTP status and message. Failing check: kafka.delete-nonexistent-resources-should-fail.

AWS: Smithy (com.amazonaws.kafka#DescribeConfiguration, GET /v1/configurations/{Arn}) models BOTH BadRequestException (HTTP 400) and NotFoundException (HTTP 404) for this operation, so the model alone does not pick one — recorded behavior only for which applies. The recording shows Amazon MSK answers an unresolvable configuration ARN on this read with HTTP 400, Error.Code "BadRequestException", Error.Message "Configuration ARN does not exist." That whole message is FIXED text for this error class: it has NO variable parts — no ARN, name, revision, region or account id is interpolated into it. MSK uses this single envelope for "the configuration you named does not resolve", which is why the configuration family's refusals (read of a deleted configuration, re-delete past the DELETING window, unissued revision) all carry it, and why the check's _MISSING_CONFIGURATION constant is ("BadRequestException", "Configuration ARN does not exist.") while every other MSK resource family in the same check uses NotFoundException "The requested resource doesn\u2019t exist."

Emulator: the handler answers HTTP 404, Error.Code "NotFoundException", with an emulator-authored, value-interpolated message of the form "No configuration exists in <region> with the ARN <arn>" (region and ARN spliced in from the request and the endpoint). So both the status (404 vs 400), the code (NotFoundException vs BadRequestException) and the message template diverge. Note the sibling handler DeleteConfiguration already refuses the same unresolvable ARN correctly, with BadRequestException and the shared crate::state::CONFIGURATION_ARN_NOT_FOUND_MESSAGE constant, so the two readers of the same state disagree about how a missing configuration is reported.

Why: a client that distinguishes an unresolvable configuration reference (400, terminal, surfaced as a validation failure) from a missing resource (404) gets the opposite classification, and SDK error-type branching on BadRequestException takes the wrong branch. Worse for lifecycle code: DeleteConfiguration is idempotent while a configuration is still DELETING, so the ONLY way to observe that a configuration has really gone is this read failing with the configuration family's not-found envelope. A poll loop that waits for 400/BadRequestException never terminates against the emulator, and one that treats 404 as "not created yet, keep polling" loops forever too. Any assertion on the documented message text also fails, because the emulator substitutes request values into a message AWS keeps fixed.

Fix: the not-found refusal branch of the DescribeConfiguration handler must construct BadRequestException (HTTP 400) with the fixed message "Configuration ARN does not exist." — reusing the same shared constant DeleteConfiguration already uses — instead of NotFoundException with a composed per-request message. This is error classification/serialization in the operation handler, not state; the malformed-ARN branch above it is already BadRequestException and is not what diverges here.

Traces: kafka.delete-nonexistent-resources-should-fail. Related but distinct open issue, which covers DescribeConfigurationRevision and NOT this operation: 20261026T085017Z-kafka-describeconfigurationrevision-refuses-an-unissued-revision-number-with-notfoundexception.md. Handler: services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration.rs lines 190-201; the correct sibling refusal is services/service-com-amazonaws-kafka-kafka/src/ops/delete_configuration.rs lines 176-186.

## Evidence
Check kafka.delete-nonexistent-resources-should-fail (assert at check line 133, via _wait_configuration_gone polling DescribeConfiguration after a successful DeleteConfiguration):
  Expected (real AWS): HTTP 400, Error.Code = "BadRequestException", Error.Message = "Configuration ARN does not exist." — entirely fixed text, no interpolated values.
  Actual (emulator):   HTTP 404, Error.Code = "NotFoundException", Error.Message = "No configuration exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:configuration/sim2real-behav-kfk-cfg-<name>/<uuid>-1" — the region and the configuration ARN are request/endpoint values spliced into an emulator-authored template; the recorded region, name and uuid above are per-run values, not part of the requirement.
  Harness assertion: AssertionError: DescribeConfiguration on an already-deleted configuration: expected ('BadRequestException', 'Configuration ARN does not exist.'), got ('NotFoundException', 'No configuration exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:configuration/...-1')

Everything the check asserts before this point PASSES: all seven never-issued-ARN refusals (DeleteCluster, DeleteTopic, DeleteConfiguration, DeleteChannel, DeleteReplicator, DeleteVpcConnection, DeleteClusterPolicy) match real MSK, including DeleteConfiguration's BadRequestException "Configuration ARN does not exist.", and the first DeleteConfiguration returns Arn plus State == "DELETING" as expected. The defect is isolated to DescribeConfiguration's not-found refusal path.

Source of the divergence: services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration.rs:190-201 returns NotFoundException with format!("No configuration exists in {region} with the ARN {}", input.arn). Contrast delete_configuration.rs:176-186, which returns BadRequestException with crate::state::CONFIGURATION_ARN_NOT_FOUND_MESSAGE for the identical lookup miss.

## Steps to Reproduce
1. kafka.create_configuration(Name=<unique>, ServerProperties=b"auto.create.topics.enable=true", KafkaVersions=["3.9.x"]) -> note Arn.
2. kafka.delete_configuration(Arn=<arn>) -> 200, {"Arn": <arn>, "State": "DELETING"}.
3. Poll kafka.describe_configuration(Arn=<arn>) until it raises, and inspect the ClientError.
4. Observe HTTP 404 / NotFoundException / "No configuration exists in <region> with the ARN <arn>". Real MSK answers HTTP 400 / BadRequestException / "Configuration ARN does not exist."
5. The same mismatch appears for a syntactically valid but never-issued configuration ARN: kafka.describe_configuration(Arn="arn:aws:kafka:<region>:<account>:configuration/<never-created-name>/<uuid>-1") -> emulator NotFoundException, MSK BadRequestException "Configuration ARN does not exist."
6. Compare with kafka.delete_configuration on the same unresolvable ARN, which already answers the correct BadRequestException "Configuration ARN does not exist." — the two handlers disagree.
