Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-revisions-append-only-immutable
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-revisions-append-only-immutable|run#20|AssertionError|assert ghost_rev[0] == "BadRequestException", f"expected BadRequestException, got {ghost_rev!r}"
Issue-Commit: 13273ef63a283393f8b97bbdcadeb04fa836ecfd

# DescribeConfigurationRevision refuses an unissued revision number with NotFoundException instead of BadRequestException "Configuration ARN does not exist." (1 cause)

> _Note: this issue was identified against the code base at commit `13273ef63a283393f8b97bbdcadeb04fa836ecfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationRevision

## Description
REOPENS 20261024T113950Z-kafka-describeconfigurationrevision-stub.md: its fix left the call refused.

Cause 1 of 1: DescribeConfigurationRevision on an existing MSK configuration ARN with a revision number the service never issued is refused with the wrong error class, status and message. Failing check: kafka.configuration-revisions-append-only-immutable.

AWS: Smithy (com.amazonaws.kafka#DescribeConfigurationRevision) models both BadRequestException (HTTP 400) and NotFoundException (HTTP 404) for this operation, so the model alone does not pick one; recorded behavior only for which applies. The recording shows that for an MSK configuration that DOES exist but holds no such revision, Amazon MSK answers HTTP 400 BadRequestException with the message "Configuration ARN does not exist." — that whole message is FIXED text for this error class (it has no variable parts: no ARN, revision number, region or id is interpolated into it). MSK deliberately uses this one envelope for "the configuration-plus-revision you named does not resolve", which is why the same (code, message) pair is also what a read of a deleted configuration answers.

Emulator: the handler answers HTTP 404 NotFoundException with an invented, value-interpolated message of the form "Configuration <arn> has no revision <n>" (observed: NotFoundException / "Configuration arn:aws:kafka:<region>:<account>:configuration/<name>/<uuid>-1 has no revision 99"). It also answers NotFoundException "No configuration exists in <region> with the ARN <arn>" when the configuration itself is absent, where AWS uses the same BadRequestException "Configuration ARN does not exist." envelope.

Why: a client that distinguishes a malformed/unresolvable configuration reference (400, retry never helps, surfaced as a validation failure) from a genuinely missing resource gets the opposite classification, and the HTTP status flips 400 -> 404. SDK error-type branching on BadRequestException (and any wait/poll loop that treats 404 as "not yet created, keep polling" while treating 400 as terminal) takes the wrong branch, and any assertion on the documented message text fails because the emulator substitutes request values into a message AWS keeps fixed.

Fix: the refusal paths of the DescribeConfigurationRevision handler — the unissued-revision branch and the unknown-configuration branch must both construct BadRequestException with the fixed message "Configuration ARN does not exist." rather than NotFoundException with a composed message. This is error-classification/serialization in the operation handler, not state.

Traces: kafka.configuration-revisions-append-only-immutable (assert at check line 894: expected BadRequestException, got ('NotFoundException', 'Configuration .../... has no revision 99')). Handler: services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration_revision.rs lines 199-224. Related but distinct open issue, which does not cover this operation: 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md.

## Evidence
Check kafka.configuration-revisions-append-only-immutable, DescribeConfigurationRevision(Arn=<existing configuration ARN>, Revision=99):
  Expected (real AWS): HTTP 400, Error.Code = "BadRequestException", Error.Message = "Configuration ARN does not exist." (message is fixed text, no interpolated values).
  Actual (emulator):   HTTP 404, Error.Code = "NotFoundException", Error.Message = "Configuration arn:aws:kafka:us-west-2:000000000000:configuration/sim2real-behav-config-append-a0cbedbe3dc8/fab3b369-5f55-4e47-8841-17c4af40cbf6-1 has no revision 99" — the ARN and the revision number are request values spliced into an emulator-authored template.
  Harness assertion: AssertionError: expected BadRequestException, got ('NotFoundException', 'Configuration arn:aws:kafka:us-west-2:000000000000:configuration/sim2real-behav-config-append-a0cbedbe3dc8/fab3b369-5f55-4e47-8841-17c4af40cbf6-1 has no revision 99')

Source of the divergence: services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration_revision.rs:213-224 returns NotFoundException for a missing revision, and :199-210 returns NotFoundException for a missing configuration.

Everything the check asserts before this point passes (create -> revision 1 ACTIVE, update -> revision 2, ListConfigurationRevisions contains {1,2}, per-revision ServerProperties byte-for-byte), so the defect is isolated to the refusal path.

## Steps to Reproduce
1. kafka.create_configuration(Name=<unique>, ServerProperties=b"auto.create.topics.enable=false", KafkaVersions=["3.6.0"]) -> note Arn, LatestRevision.Revision == 1.
2. kafka.update_configuration(Arn=<arn>, ServerProperties=b"auto.create.topics.enable=true\nnum.partitions=2") -> LatestRevision.Revision == 2.
3. kafka.describe_configuration_revision(Arn=<arn>, Revision=99)  # a revision number the service never issued
4. Observe the ClientError: emulator gives HTTP 404 / NotFoundException / "Configuration <arn> has no revision 99". Real MSK gives HTTP 400 / BadRequestException / "Configuration ARN does not exist."
5. Same mismatch on a configuration ARN that does not exist at all: emulator answers NotFoundException "No configuration exists in <region> with the ARN <arn>" where MSK answers BadRequestException "Configuration ARN does not exist."
