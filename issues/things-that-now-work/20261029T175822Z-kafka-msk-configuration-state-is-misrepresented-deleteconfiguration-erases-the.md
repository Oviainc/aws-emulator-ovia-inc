Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-update-rejected-while-deleting kafka.configuration-description-empty-vs-absent-preserved
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-update-rejected-while-deleting|run#16|AssertionError|assert msg == _DELETING_MSG, (
Issue-Key: kafka.configuration-description-empty-vs-absent-preserved|run#7|AssertionError|assert "Description" not in described_absent, (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# MSK configuration state is misrepresented: DeleteConfiguration erases the record synchronously, and DescribeConfiguration fabricates an empty Description (2 causes)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfiguration

## Description
TWO distinct root causes in how configuration state is stored and rendered. Both must be closed.

Cause 1 of 2 - DeleteConfiguration erases the record instead of transitioning it to DELETING, so every subsequent answer is a lookup miss.
AWS: Smithy kafka#DeleteConfiguration answers with a ConfigurationState, and kafka#UpdateConfiguration (PUT /v1/configurations/{Arn}) models BadRequestException (400) and NotFoundException (404). Recorded behavior: DeleteConfiguration is ASYNCHRONOUS and returns State 'DELETING'. During that delete window the configuration is still a visible resource - ListConfigurations still enumerates it, with State 'DELETING' - and an UpdateConfiguration against it is refused HTTP 400 / 'BadRequestException' with Error.Message exactly "This operation is only valid for resources that are in one of the following states :[ACTIVE]" (fixed text, including the space before the colon; the bracketed list is the set of states the operation permits). Only ONCE the asynchronous delete has completed - the name has left ListConfigurations - does the SAME UpdateConfiguration become a lookup miss, still HTTP 400 / 'BadRequestException' but with Error.Message "Configuration ARN does not exist." The two messages discriminate a state-machine refusal from a lookup miss, and that distinction is the contract.
Emulator: it reports the state-machine refusal as a lookup miss - the UpdateConfiguration issued immediately after DeleteConfiguration already answers 'Configuration ARN does not exist.' That is the signature of a handler that removes the record synchronously rather than marking it DELETING, which also means the configuration disappears from ListConfigurations at once and no DELETING state is ever observable.
Why: a caller that polls a deletion to completion (list or describe until the name is gone / the state settles) sees it vanish instantaneously and cannot distinguish 'delete accepted, in flight' from 'never existed'; worse, a caller that retries a racing update is told its ARN is wrong rather than that the resource is busy, so it re-creates a configuration under a new name instead of waiting.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/delete_configuration.rs and the configuration state it owns - mark the record DELETING, keep it enumerable by list_configurations.rs and readable by describe_configuration.rs with State 'DELETING' until the asynchronous delete completes, and have update_configuration.rs apply the ACTIVE-only state gate (with the states message) BEFORE it reports a lookup miss. No new revision may be written by the refused update.
Traces: kafka.configuration-update-rejected-while-deleting.

Cause 2 of 2 - DescribeConfiguration invents a Description member that the configuration does not have.
AWS: Smithy kafka#CreateConfiguration takes an OPTIONAL Description (jsonName 'description'); kafka#DescribeConfiguration's output likewise models Description as optional. Recorded behavior: a configuration created WITHOUT a Description is described back with the Description member ABSENT from the response - absent is distinct from present-and-empty, which is what a configuration created with Description='' returns.
Emulator: DescribeConfiguration emits Description with the value '' for a configuration created without one, collapsing 'no description' into 'empty description'.
Why: this is a serializer defect that destroys information a client can legitimately branch on - round-tripping a described configuration back through CreateConfiguration/UpdateConfiguration now sends an explicit empty Description where the caller sent none, and any config-drift comparison reports a spurious difference. Optional members must be omitted when unset, not defaulted.
Fix: the output serialization in services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration.rs (and the configuration record it reads) - store Description as an option and skip the member when it is unset instead of substituting the empty string.
Traces: kafka.configuration-description-empty-vs-absent-preserved.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

Cause 1 - kafka.configuration-update-rejected-while-deleting (mismatch):
  create_configuration -> ACTIVE; update_configuration -> revision 2; delete_configuration(Arn=<it>) -> State 'DELETING'; then immediately update_configuration(Arn=<it>, ServerProperties=...)
  expected ('BadRequestException', 400, 'This operation is only valid for resources that are in one of the following states :[ACTIVE]'), with the configuration still listed by ListConfigurations carrying State 'DELETING'; and only AFTER the delete completes the same call must answer ('BadRequestException', 400, 'Configuration ARN does not exist.')
  actual   AssertionError: the refusal must be the state-machine refusal ('This operation is only valid for resources that are in one of the following states :[ACTIVE]') -- an implementation that erased the record synchronously reports it as a lookup miss instead: 'Configuration ARN does not exist.'

Cause 2 - kafka.configuration-description-empty-vs-absent-preserved (mismatch):
  create_configuration(Name=<new>, ServerProperties=...) with NO Description, then describe_configuration(Arn=<it>)
  expected the response to have no 'Description' key at all
  actual   AssertionError: DescribeConfiguration fabricated a Description for a configuration created without one: ''

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.create_configuration(Name='victim', KafkaVersions=['3.9.x'], ServerProperties=b'auto.create.topics.enable=true\n'); poll describe_configuration until State 'ACTIVE'; kafka.update_configuration(Arn=<it>, ServerProperties=b'auto.create.topics.enable=true\nnum.partitions=2\n') -> LatestRevision.Revision 2.
3. kafka.delete_configuration(Arn=<it>) -> expect State 'DELETING'.
4. Immediately kafka.update_configuration(Arn=<it>, ServerProperties=b'...num.partitions=9\n') -> expect HTTP 400 'BadRequestException' with Message 'This operation is only valid for resources that are in one of the following states :[ACTIVE]'. Observe 'Configuration ARN does not exist.'
5. While the delete is in flight, kafka.list_configurations() -> the configuration must still be listed, with State 'DELETING'. Confirm no third revision was written.
6. Poll until the name leaves list_configurations, then repeat the same update_configuration -> expect HTTP 400 'BadRequestException' with Message 'Configuration ARN does not exist.'
7. Cause 2: kafka.create_configuration(Name='nodesc', ServerProperties=b'auto.create.topics.enable=true\n') with no Description argument, then kafka.describe_configuration(Arn=<it>) -> the response must contain no 'Description' member. Observe Description == ''. For contrast, create one with Description='' and confirm that one DOES return Description ''.
8. Re-replay kafka.configuration-update-rejected-while-deleting and kafka.configuration-description-empty-vs-absent-preserved.
