Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-describe-malformed-arn-rejected kafka.channel-invalid-name-pattern-rejected kafka.batch-disassociate-scram-secret-malformed-arn-rejected kafka.configuration-arn-region-scope-enforced
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-describe-malformed-arn-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: kafka.channel-invalid-name-pattern-rejected|run#18|AssertionError|assert malformed_code == "AccessDeniedException", (
Issue-Key: kafka.batch-disassociate-scram-secret-malformed-arn-rejected|run#5|AssertionError|assert malformed_code == "NotFoundException", repr(malformed_code)
Issue-Key: kafka.configuration-arn-region-scope-enforced|run#15|AssertionError|assert read_code == "NotFoundException", (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# MSK ARN handling: malformed and cross-region ARNs are refused as BadRequestException where MSK answers AccessDenied/NotFound (4 causes)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
FOUR distinct root causes sharing one shape: the emulator runs its own ARN-syntax validator first and answers BadRequestException, where MSK reaches a different, earlier or later gate and answers a different Error.Code. The status/code a client dispatches on is wrong in all four. All four must be closed.

(The open issue 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md describes this ordering for DescribeTopicPartitions - Cause 4 there, and the suite marks kafka.describe-topic-partitions-malformed-name-rejected as owned by it. The four causes below are different operations whose checks that filing does not carry.)

Cause 1 of 4 - DescribeCluster/DescribeClusterV2 with a ClusterArn whose resource segment is not of the form 'cluster/<name>/<uuid>'.
AWS: Smithy kafka#DescribeCluster (GET /v1/clusters/{ClusterArn}) models BadRequestException (400) and NotFoundException (404). Recorded behavior: authorization is evaluated BEFORE any resource lookup, so an ARN that cannot be matched against the caller's resource-scoped permissions is refused with Error.Code 'AccessDeniedException' - not with an ARN-syntax 400. A well-formed-but-nonexistent ARN passes that gate and reaches NotFoundException. Two ordered gates, two codes.
Emulator: Error.Code 'BadRequestException' for the malformed ARN, i.e. its own ARN parser answers before authorization is consulted.
Why: a client that distinguishes 'you may not touch this' (AccessDeniedException - escalate to IAM) from 'your request is malformed' (BadRequestException - fix the input) is routed to the wrong remedy, and least-privilege tests cannot be written against the emulator.
Fix: in services/service-com-amazonaws-kafka-kafka/src/ops/describe_cluster.rs / describe_cluster_v2.rs, let the request-authorization layer run on the raw ARN before the handler's ARN parsing, so an unmatchable ARN becomes AccessDeniedException.
Traces: kafka.cluster-describe-malformed-arn-rejected.

Cause 2 of 4 - the channel surface with a malformed ClusterArn.
AWS: Smithy kafka#CreateChannel (POST /v1/clusters/{ClusterArn}/channels) models BadRequestException (400), NotFoundException (404) and the auth errors. Recorded behavior: exactly as Cause 1 - a ClusterArn whose resource segment is not 'cluster/<name>/<uuid>' is refused by resource-scoped authorization with Error.Code 'AccessDeniedException' before the channel-name pattern or the cluster lookup is evaluated.
Emulator: 'BadRequestException'.
Why: same as Cause 1; it also shows the gate order is wrong per-handler rather than once, so fixing only DescribeCluster leaves the channel surface diverging.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_channel.rs (and the sibling channel handlers) must consult authorization on the path ClusterArn before parsing it.
Traces: kafka.channel-invalid-name-pattern-rejected.

Cause 3 of 4 - BatchDisassociateScramSecret does not run a separate ARN-shape pass on SecretArnList.
AWS: Smithy kafka#BatchDisassociateScramSecret (PATCH /v1/clusters/{ClusterArn}/scram-secrets) models BadRequestException (400) and NotFoundException (404). Recorded behavior: MSK runs NO separate secret-ARN syntax validation for this operation - a plainly non-ARN SecretArnList entry and a well-formed-but-never-associated secret ARN are funnelled through the SAME contract (the API's own documentation: the error is raised when 'ClusterArn does not exist, or a SecretArn in the list is not currently associated'). Both are refused with Error.Code 'NotFoundException' and Error.Message exactly "The requested resource doesn\u2019t exist." (U+2019 apostrophe, all fixed text) - and identically to each other, which is the contract.
Emulator: 'BadRequestException' for the non-ARN entry, i.e. it adds a shape-validation pass MSK does not have, splitting one contract into two.
Why: a client cleaning up SCRAM associations handles 'not currently associated' as benign and a 400 as a bug in its own request; the emulator makes the benign case look like the bug. Note this is the OPPOSITE direction from BatchAssociateScramSecret, where MSK DOES validate the secret ARN shape (400 'The provided secret ARN is invalid.') - the two batch operations must not share one validator.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/batch_disassociate_scram_secret.rs - drop the SecretArnList syntax pass and treat an unparseable entry as 'not associated', returning the fixed NotFoundException. The open issues 20261027T231320Z-kafka-scram-secret-operations-are-over-permissive-no.md and 20261029T072939Z-kafka-scram-secret-refusals-no-cluster-state-gate.md cover the UnprocessedScramSecrets per-item behaviour and the cluster-state gates; neither covers this gate.
Traces: kafka.batch-disassociate-scram-secret-malformed-arn-rejected.

Cause 4 of 4 - the configuration ARN's REGION segment is not honoured as a scope.
AWS: Smithy kafka#DescribeConfiguration (GET /v1/configurations/{Arn}), kafka#DescribeConfigurationRevision and kafka#UpdateConfiguration (PUT /v1/configurations/{Arn}) all model BadRequestException (400) and NotFoundException (404). Recorded behavior, with exactly one ARN segment changed: a well-formed configuration ARN whose region segment names a DIFFERENT real region than the endpoint being called is refused HTTP 404 / Error.Code 'NotFoundException' with Message "The requested resource doesn\u2019t exist." - on the read, on the revision read AND on the write; a region segment that is not a region at all (e.g. 'xx-fake-1') is a distinct, earlier refusal: HTTP 400 / 'BadRequestException' with Message 'One or more of the parameters are not valid.' So the ARN's region must be matched against the request's region as part of resolution, and only an unparseable region is an input error.
Emulator: 'BadRequestException' (HTTP 400) for the other-real-region ARN - it collapses 'this ARN belongs to another region' into its ARN-validation/lookup-miss 400 instead of a 404 not-found. (A fixed issue, 20261026T235948Z-kafka-describeconfiguration-answers-404-notfoundexception-with-an-invented.md, correctly made a same-region lookup miss a 400 'Configuration ARN does not exist.'; the cross-region case is a different gate and must be 404 NotFoundException.)
Why: a client that accidentally carries a configuration ARN across regions cannot tell 'wrong region' from 'malformed ARN', and an emulator that resolves the ARN to the LOCAL configuration anyway would let a cross-region UpdateConfiguration land on the wrong resource - the check also asserts the local configuration's revision 1 is untouched.
Fix: make the region segment part of configuration resolution in services/service-com-amazonaws-kafka-kafka/src/ops/describe_configuration.rs, describe_configuration_revision.rs and update_configuration.rs (one shared ARN resolver, so the guard is on the ARN and not on one operation), keeping an unparseable region as the earlier 400.
Traces: kafka.configuration-arn-region-scope-enforced.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

Cause 1 - kafka.cluster-describe-malformed-arn-rejected (mismatch):
  describe_cluster(ClusterArn=<resource segment not 'cluster/<name>/<uuid>'>)
  expected Error.Code 'AccessDeniedException'
  actual   AssertionError: expected AccessDeniedException, got 'BadRequestException'

Cause 2 - kafka.channel-invalid-name-pattern-rejected (mismatch):
  the channel operation with a malformed ClusterArn
  expected Error.Code 'AccessDeniedException'
  actual   AssertionError: expected AccessDeniedException for a malformed ClusterArn, got=BadRequestException

Cause 3 - kafka.batch-disassociate-scram-secret-malformed-arn-rejected (mismatch):
  batch_disassociate_scram_secret(ClusterArn=<well-formed nonexistent>, SecretArnList=['not-an-arn-at-all'])
  expected ('NotFoundException', 'The requested resource doesn\u2019t exist.')
  actual   AssertionError: 'BadRequestException'
  (the same check then asserts the well-formed-but-missing secret ARN produces the identical code and message, i.e. one funnelled contract)

Cause 4 - kafka.configuration-arn-region-scope-enforced (mismatch):
  create_configuration -> ACTIVE, then describe_configuration(Arn=<the same ARN with only the region segment rewritten to another real region>)
  expected HTTP 404, Error.Code 'NotFoundException', Message 'The requested resource doesn\u2019t exist.'  (and the same for describe_configuration_revision and update_configuration)
  actual   AssertionError: DescribeConfiguration with a us-east-2 ARN against the us-west-2 endpoint must be NotFoundException, got BadRequestException (HTTP 400); resolving it to the local configuration means the region segment is being ignored
  (control that must keep working: the same ARN with region segment 'xx-fake-1' -> HTTP 400 BadRequestException, Message 'One or more of the parameters are not valid.')

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.describe_cluster(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/not-a-valid-shape') -> expect Error.Code 'AccessDeniedException'; then the same call with a well-formed never-issued ARN -> expect 'NotFoundException'. Observe BadRequestException for the first.
3. Cause 2: call the channel operation with ClusterArn='arn:aws:kafka:<region>:<account>:cluster/not-a-valid-shape' -> expect 'AccessDeniedException'. Observe BadRequestException.
4. Cause 3: kafka.batch_disassociate_scram_secret(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/never-issued/00000000-0000-0000-0000-000000000000-1', SecretArnList=['not-an-arn-at-all']) -> expect 404 NotFoundException, Message 'The requested resource doesn\u2019t exist.'; repeat with SecretArnList=['arn:aws:secretsmanager:<region>:<account>:secret:AmazonMSK_does-not-exist-x1y2z3'] -> expect the IDENTICAL code and message.
5. Cause 4: kafka.create_configuration(Name='arnscope', KafkaVersions=['3.9.x'], ServerProperties=b'auto.create.topics.enable=true\n'); take its Arn and rewrite ONLY the region segment to another real region; call describe_configuration, describe_configuration_revision(Revision=1) and update_configuration on the rewritten ARN -> each must be HTTP 404 'NotFoundException' with Message 'The requested resource doesn\u2019t exist.'. Then rewrite the region segment to 'xx-fake-1' and describe_configuration -> expect HTTP 400 'BadRequestException', Message 'One or more of the parameters are not valid.'. Finally describe the ORIGINAL ARN -> State ACTIVE, LatestRevision.Revision still 1, and revision 1's ServerProperties must not contain num.partitions.
6. Re-replay every check id in trace_ids.
