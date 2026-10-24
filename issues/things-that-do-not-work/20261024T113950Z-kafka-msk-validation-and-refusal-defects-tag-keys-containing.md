Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.e2e-scram-secret-auth-grant-and-revoke kafka.topic-update-controller-moved-error kafka.configuration-create-rejected-for-unsupported-kafka-version kafka.replicator-read-paths-report-clean-miss
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateCluster|BadRequestException
Issue-Key: kafka.configuration-create-rejected-for-unsupported-kafka-version|_error_code#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp!r}")
Issue-Key: kafka.replicator-read-paths-report-clean-miss|run#8|AssertionError|assert describe_err == expected, f"describe: expected {expected!r}, got {describe_err!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# MSK validation and refusal defects: tag keys containing ':' are rejected, CreateConfiguration accepts an unsupported KafkaVersions entry, and replicator not-found messages do not name the offending parameter (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
THREE distinct root causes in the kafka (Amazon MSK) handlers. All three must be closed.

Cause 1 of 3 — a tag key containing ':' is refused, so resources real AWS tags cannot be created.
AWS: Smithy kafka#CreateCluster's Tags member targets __mapOf__string with NO @pattern and no @length on the key, so the model imposes no character set; the governing rule is AWS's own tagging rule (AWS General Reference, "Tagging AWS resources": a tag key may contain Unicode letters, digits, whitespace and the characters + - = . _ : / @), which INCLUDES the colon. Recorded behavior: CreateCluster carrying the tag key 'sim2real:managed' is accepted, HTTP 200 with the cluster in CREATING.
Emulator: CreateCluster is refused with HTTP 400, Error.Code 'BadRequestException' (message example: "Tag key 'sim2real:managed' contains the unsupported character ':'; a tag key must consist of Unicode letters, digits, white space and the special characters _ . / = + - @."). services/service-com-amazonaws-kafka-kafka/src/state.rs `is_tag_key_character` (~line 2457) admits only `_ . / = + - @` and omits ':', and the message it builds enumerates that same short set.
Why: namespaced tag keys ('team:owner', 'cost-center:1234', and every AWS-generated key such as 'aws:cloudformation:stack-name') are the normal convention. Against the emulator any create or TagResource that carries one is refused outright, so a client cannot provision tagged resources at all — and because the refusal is a modeled BadRequestException it looks like the client's own mistake.
Fix: services/service-com-amazonaws-kafka-kafka/src/state.rs — add ':' to the admitted tag-key character set in `is_tag_key_character` and correct the message's enumeration to the documented set; apply it uniformly to CreateCluster, CreateClusterV2, CreateReplicator, CreateConfiguration and TagResource, which share this validator.
Traces: kafka.e2e-scram-secret-auth-grant-and-revoke, kafka.topic-update-controller-moved-error.

Cause 2 of 3 — CreateConfiguration accepts a KafkaVersions entry that names no Apache Kafka version.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations) models BadRequestException (httpError 400). Recorded behavior: KafkaVersions=['0.0.1-does-not-exist'] is refused with HTTP 400, Error.Code 'BadRequestException', and nothing is persisted (the name never appears in ListConfigurations). The message embeds MSK's whole version catalogue in an internal order and is therefore an example only, not a requirement. The same call with a supported version ('3.9.x') returns 200 with State 'ACTIVE' and LatestRevision.Revision 1.
Emulator: the call SUCCEEDS — HTTP 200 with Arn, State 'ACTIVE', LatestRevision.Revision 1 — so the unsupported version is persisted as a usable configuration.
Why: this is the permissive direction. A client gets a configuration that real MSK would never have created, and the failure moves to CreateCluster/UpdateClusterConfiguration (or to a real account) instead of to the call that was wrong. The emulator already holds the version catalogue every version-aware operation answers from (KAFKA_VERSION_CATALOG in src/state.rs), so the input is validatable against the same list ListKafkaVersions reports.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs — validate every KafkaVersions entry against KAFKA_VERSION_CATALOG before anything is stored and return the modeled BadRequestException, leaving the store untouched.
Traces: kafka.configuration-create-rejected-for-unsupported-kafka-version.

Cause 3 of 3 — the replicator read/delete not-found refusal does not follow MSK's message form; it leaks the region and the full ARN instead of naming the offending parameter.
AWS: Smithy kafka#DescribeReplicator / DeleteReplicator model NotFoundException (httpError 404). Recorded behavior: a well-formed replicator ARN that was never issued is refused with HTTP 404, Error.Code 'NotFoundException' and the request-derived message 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.' — the rule being MSK's standard refusal form, which names the INPUT MEMBER at fault (replicatorArn) followed by the fixed sentence 'The requested resource doesn\u2019t exist.', and nothing else. That rule, not the recorded characters, is the requirement.
Emulator: the code is right (NotFoundException) but the message is a different sentence built from the service's own state — 'No replicator exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:replicator/<name>/00000000-0000-0000-0000-000000000000-1' — naming the region and echoing the whole ARN rather than the parameter.
Why: a client that keys on the refusal form (log/alert matchers, and any assertion that the refusal names the parameter it passed) does not recognize the emulator's refusal, and the message discloses the serving region as part of a client error where AWS discloses only the member at fault. Lower severity than the two above: the HTTP status and the error code already match.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/describe_replicator.rs and src/ops/delete_replicator.rs (or the shared not-found message builder in src/state.rs) — build the NotFoundException message in MSK's 'Invalid parameter <member>: The requested resource doesn\u2019t exist.' form from the input member that failed to resolve.
Traces: kafka.replicator-read-paths-report-clean-miss.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 11/85 passed; kafka 11/84.

Cause 1 — kafka.e2e-scram-secret-auth-grant-and-revoke and kafka.topic-update-controller-moved-error, identical shape:
  kafka.create_cluster(ClusterName=…, KafkaVersion='3.9.x', NumberOfBrokerNodes=…, BrokerNodeGroupInfo={…}, Tags={'sim2real:managed': 'true', …})
  expected: HTTP 200, cluster in CREATING, the tag readable back verbatim via ListTagsForResource
  actual:   botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateCluster operation: Tag key 'sim2real:managed' contains the unsupported character ':'; a tag key must consist of Unicode letters, digits, white space and the special characters _ . / = + - @.

Cause 2 — kafka.configuration-create-rejected-for-unsupported-kafka-version:
  kafka.create_configuration(Name='sim2real-msk-cfg-bad-…', KafkaVersions=['0.0.1-does-not-exist'], ServerProperties=b'auto.create.topics.enable=true\n')
  expected: HTTP 400, Error.Code 'BadRequestException', and the name absent from ListConfigurations
  actual:   AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, …}, 'Arn': 'arn:aws:kafka:us-west-2:000000000000:configuration/sim2real-msk-cfg-bad-e31bdf1af6ce/fbe0c64d-…-1', 'LatestRevision': {'Revision': 1, …}, 'Name': 'sim2real-msk-cfg-bad-e31bdf1af6ce', 'State': 'ACTIVE'}

Cause 3 — kafka.replicator-read-paths-report-clean-miss:
  kafka.describe_replicator(ReplicatorArn='arn:aws:kafka:us-west-2:000000000000:replicator/sim2real-behav-nonexistent-repl-…/00000000-0000-0000-0000-000000000000-1')
  expected: ('NotFoundException', 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.')
  actual:   ('NotFoundException', 'No replicator exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:replicator/sim2real-behav-nonexistent-repl-ca87067999b7/00000000-0000-0000-0000-000000000000-1')
  (the same assertion applies to delete_replicator on the same ARN)

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.create_cluster(ClusterName='tagcheck', KafkaVersion='3.9.x', NumberOfBrokerNodes=2, BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':[<two subnets in distinct AZs of the request's region>],'StorageInfo':{'EbsStorageInfo':{'VolumeSize':10}}}, Tags={'sim2real:managed':'true'}) -> expect HTTP 200 and the tag readable back by kafka.list_tags_for_resource; observe HTTP 400 BadRequestException naming ':' as unsupported. Repeat with kafka.tag_resource(ResourceArn=<any cluster>, Tags={'team:owner':'x'}).
3. Cause 2: kafka.create_configuration(Name='cfg-bad', KafkaVersions=['0.0.1-does-not-exist'], ServerProperties=b'auto.create.topics.enable=true\n') -> expect HTTP 400 BadRequestException and 'cfg-bad' absent from kafka.list_configurations(); observe HTTP 200 with State 'ACTIVE' and the configuration persisted. Then the same call with KafkaVersions=['3.9.x'] -> expect 200, State 'ACTIVE', LatestRevision.Revision 1.
4. Cause 3: kafka.describe_replicator(ReplicatorArn='arn:aws:kafka:<region>:<account>:replicator/does-not-exist/00000000-0000-0000-0000-000000000000-1') and kafka.delete_replicator with the same ARN -> expect HTTP 404 NotFoundException whose message names the offending parameter in MSK's 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.' form; observe a message naming the region and echoing the ARN.
5. Re-replay the 4 check ids in trace_ids.
