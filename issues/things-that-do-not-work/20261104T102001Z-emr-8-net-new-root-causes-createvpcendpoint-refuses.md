Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.action-on-failure-continue-then-terminate-cluster emr.add-steps-to-terminated-cluster-rejected emr.auto-termination-policy-roundtrip emr.bootstrap-action-failure-terminates-with-errors emr.cluster-reaches-waiting-with-running-instances emr.core-fleet-zero-target-capacity-rejected emr.core-instance-group-resize-adds-a-real-ec2-instance emr.deploy-cfn-cluster-executes-step emr.e2e-cluster-create-step-execute-terminate-flow emr.ebs-configuration-provisions-the-requested-real-volumes emr.emr-run-step-spark emr.emr-step-args-are-passed-verbatim-to-the-application emr.get-session-endpoint-returns-reachable-endpoint emr.hdfs-write-survives-the-step-boundary emr.list-clusters-time-filters emr.managed-scaling-policy-put-then-get-roundtrip emr.modify-instance-fleet-target-capacity-then-list-reflects-change emr.multi-master-instance-groups-round-trip emr.notebook-execution-lifecycle-transitions emr.op-cluster-read-ops-reject-unknown-cluster emr.op-run-job-flow-unknown-release-label-fails emr.op-tag-ops-reject-unknown-resource emr.persistent-app-ui-presigned-url-flow emr.put-auto-scaling-policy-malformed-rule-rejected emr.session-lifecycle-start-get-terminate emr.set-unhealthy-node-replacement-updates-cluster-configuration emr.spark-cluster-applications-resolve-release-versions emr.step-added-while-cluster-starting-queues-then-runs emr.termination-protection-blocks-terminate-job-flows emr.unicode-tag-value-round-trip emr.deploy-emr-studio-via-cloudformation-session-mapping-usable emr.emr-block-public-access-rejects-a-cluster-with-an-open-security-group emr.studio-session-mapping-unicode-identity-preserved emr.studio-session-mapping-unicode-identity-name-preserved emr.studio-session-mapping-missing-identity-rejected emr.e2e-studio-session-start-execute-terminate-flow emr.studio-session-mapping-invalid-identity-rejected emr.studio-session-mapping-invalid-session-policy-arn-rejected emr.studio-session-mapping-delete-then-get-not-found emr.spark-step-partitioned-write-lands-hive-layout-in-s3 emr.spark-step-with-a-missing-input-path-fails-and-writes-no-output emr.spark-bigint-aggregate-beyond-float64-precision-is-exact
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|CreateVpcEndpoint|NotImplemented
Issue-Key: emr|CreateStack|ValidationError
Issue-Key: emr.emr-block-public-access-rejects-a-cluster-with-an-open-security-group|run#19|AssertionError|assert state == "TERMINATED_WITH_ERRORS", (
Issue-Key: emr.studio-session-mapping-unicode-identity-preserved|run#12|KeyError|assert mapping["IdentityId"] == group_id, (
Issue-Key: emr|GetStudioSessionMapping|InvalidRequestException
Issue-Key: emr.studio-session-mapping-missing-identity-rejected|run#23|KeyError|assert mapping["IdentityId"] == group_id
Issue-Key: emr.e2e-studio-session-start-execute-terminate-flow|run#20|KeyError|assert mapping["IdentityId"] == group_id, f"expected mapping IdentityId {group_id!r}, got {mapping['IdentityId']!r}"
Issue-Key: emr.studio-session-mapping-invalid-identity-rejected|_error_code#11|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: emr.studio-session-mapping-invalid-session-policy-arn-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: emr.studio-session-mapping-delete-then-get-not-found|run#13|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: emr.spark-step-partitioned-write-lands-hive-layout-in-s3|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: emr.spark-step-with-a-missing-input-path-fails-and-writes-no-output|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: emr.spark-bigint-aggregate-beyond-float64-precision-is-exact|_poll#15|Timeout|time.sleep(interval_s)
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# EMR: 8 net-new root causes — CreateVpcEndpoint refuses route-table associations (blocks ~130 emr checks), CFN does not know the EMR Studio resource types, block public access is not enforced, and Studio session mappings are unresolved/unvalidated

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint, CreateStack, RunJobFlow, CreateStudioSessionMapping, GetStudioSessionMapping, AddJobFlowSteps

## Description
Eight distinct root causes clustered from this run's emr failures. Every cause below is numbered and carries its own failing check ids; a fixer must close all eight before submitting.

=== Cause 1 of 8: EC2 CreateVpcEndpoint refuses any request that names RouteTableIds with 501 NotImplemented, so ~130 emr checks die in VPC setup before the first EMR call ===
AWS: Smithy (com.amazonaws.ec2#AmazonEC2): CreateVpcEndpoint input carries RouteTableIds (VpcEndpointRouteTableIdList, xmlName RouteTableId) next to VpcId and ServiceName; it is a normal member of a Gateway-endpoint request. AWS answers HTTP 200 with CreateVpcEndpointResult.VpcEndpoint whose RouteTableIds echo the associated route tables, and installs the prefix-list route in each named table. The operation models no error for this input; a refusal would be a 400 with a Client.* code, never 501 NotImplemented.
Emulator: HTTP 501, error code 'NotImplemented', message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'. The whole message is emulator-authored fixed text — AWS emits nothing like it for this operation; no part of it comes from the request.
Why: every EMR scenario whose estate places an S3 gateway endpoint on the VPC's route tables aborts in setup, so no EMR control- or data-plane behaviour behind it is reachable, and a client doing the same (AWS-accepted) wiring sees a non-retryable 501.
Fix: the ec2 crate's CreateVpcEndpoint handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs) — associate the named route tables and serialize RouteTableIds on the returned VpcEndpoint instead of refusing.
Note: open issues 20261104T080454Z-emr-events-sqs-ec2-createvpcendpoint-refuses-any-request-carrying-routetableids-with.md, 20261104T082656Z-emr-glue-ec2-createvpcendpoint-refuses-any-request-carrying.md, 20261104T075553Z-ec2-emr-iam-createvpcendpoint-refuses-a-gateway-endpoint.md and 20261104T055008Z-emr-glue-s3-2-net-new-root-causes.md describe this same refusal for checks under OTHER service headings; none of them carries the emr check keys listed here, so the emr checks below must be re-replayed to confirm the fix.
Traces (same NotImplemented refusal in every one; the ids below are the representative set to re-replay, and the identical refusal accounts for every other [error] emr failure in this run that is not named under causes 2-8): emr.action-on-failure-continue-then-terminate-cluster, emr.add-steps-to-terminated-cluster-rejected, emr.auto-termination-policy-roundtrip, emr.bootstrap-action-failure-terminates-with-errors, emr.cluster-reaches-waiting-with-running-instances, emr.core-fleet-zero-target-capacity-rejected, emr.core-instance-group-resize-adds-a-real-ec2-instance, emr.deploy-cfn-cluster-executes-step, emr.e2e-cluster-create-step-execute-terminate-flow, emr.ebs-configuration-provisions-the-requested-real-volumes, emr.emr-run-step-spark, emr.emr-step-args-are-passed-verbatim-to-the-application, emr.get-session-endpoint-returns-reachable-endpoint, emr.hdfs-write-survives-the-step-boundary, emr.list-clusters-time-filters, emr.managed-scaling-policy-put-then-get-roundtrip, emr.modify-instance-fleet-target-capacity-then-list-reflects-change, emr.multi-master-instance-groups-round-trip, emr.notebook-execution-lifecycle-transitions, emr.op-cluster-read-ops-reject-unknown-cluster, emr.op-run-job-flow-unknown-release-label-fails, emr.op-tag-ops-reject-unknown-resource, emr.persistent-app-ui-presigned-url-flow, emr.put-auto-scaling-policy-malformed-rule-rejected, emr.session-lifecycle-start-get-terminate, emr.set-unhealthy-node-replacement-updates-cluster-configuration, emr.spark-cluster-applications-resolve-release-versions, emr.step-added-while-cluster-starting-queues-then-runs, emr.termination-protection-blocks-terminate-job-flows, emr.unicode-tag-value-round-trip.

=== Cause 2 of 8: CloudFormation does not know the EMR Studio resource types, so a template declaring AWS::EMR::Studio / AWS::EMR::StudioSessionMapping is refused at CreateStack ===
AWS: recorded behavior only (the EMR Smithy model does not describe CloudFormation). AWS::EMR::Studio and AWS::EMR::StudioSessionMapping are published CloudFormation resource types; CreateStack with a template declaring them answers HTTP 200 with a StackId and provisions the Studio and its session mapping, and the stack's Outputs resolve !Ref Studio to the es-* Studio id.
Emulator: HTTP 400, error code 'ValidationError', message 'Template format error: Unrecognized resource types: [AWS::EMR::Studio, AWS::EMR::StudioSessionMapping]'. Fixed text: 'Template format error: Unrecognized resource types: [' + the list of types taken from the template + ']'. AWS only emits that sentence for types it genuinely does not support.
Why: an infrastructure-as-code client cannot deploy EMR Studio at all; the stack never exists, so nothing downstream of it (studio id, session mapping usability) is reachable.
Fix: the CloudFormation crate's resource-type registry and provisioners — register AWS::EMR::Studio and AWS::EMR::StudioSessionMapping and route their lifecycle to the EMR crate's CreateStudio / CreateStudioSessionMapping.
Traces: emr.deploy-emr-studio-via-cloudformation-session-mapping-usable.

=== Cause 3 of 8: EMR block public access is not enforced — a cluster whose security group is open to 0.0.0.0/0 on a non-permitted port starts normally ===
AWS: Smithy (com.amazonaws.emr#ElasticMapReduce) BlockPublicAccessConfiguration documents the rule: 'When BlockPublicSecurityGroupRules is set to true, Amazon EMR prevents cluster creation if one of the cluster's security groups has a rule that allows inbound traffic from 0.0.0.0/0 or ::/0 on a port, unless the port is specified as an exception using PermittedPublicSecurityGroupRuleRanges.' Recorded behavior: RunJobFlow is accepted, then the cluster is refused during provisioning and settles in TERMINATED_WITH_ERRORS with a StateChangeReason naming the blocked public security-group rule.
Emulator: the cluster ignores the account's block-public-access configuration and reaches state 'WAITING' with StateChangeReason absent (None).
Why: a client relying on EMR block public access as an account guardrail gets a cluster that really does run with a world-open port; a negative test of the guardrail can never fail, so the protection is silently absent.
Fix: the emr crate's RunJobFlow handler / cluster-lifecycle state machine — consult the stored BlockPublicAccessConfiguration (BlockPublicSecurityGroupRules plus PermittedPublicSecurityGroupRuleRanges) against the cluster's EC2 security-group ingress rules and drive the cluster to TERMINATED_WITH_ERRORS with the corresponding StateChangeReason.
Traces: emr.emr-block-public-access-rejects-a-cluster-with-an-open-security-group.

=== Cause 4 of 8: Studio session mappings are stored under whichever identifier the caller supplied and are never resolved in IAM Identity Center, so SessionMapping omits IdentityId and a by-IdentityId read of a by-name mapping is refused ===
AWS: Smithy SessionMappingDetail models StudioId, IdentityId ('The globally unique identifier (GUID) of the user or group'), IdentityName, IdentityType, SessionPolicyArn, CreationTime and LastModifiedTime. CreateStudioSessionMapping takes IdentityId OR IdentityName and resolves the identity in the Studio's Identity Center instance, so GetStudioSessionMapping returns BOTH IdentityId and IdentityName whichever one was supplied, and the same mapping is readable by either identifier.
Emulator: GetStudioSessionMapping's SessionMapping carries no IdentityId member at all (the client raises KeyError: 'IdentityId'), and a mapping created with IdentityName only cannot be read by IdentityId — that read is refused 400 InvalidRequestException with the emulator-authored message "Studio '<studio-id>' has no session mapping for <IdentityType> '<identity-id>'." (fixed text "Studio '" ... "' has no session mapping for " ... ; the studio id, identity type and identity value come from the request).
Why: a client that maps an identity by name and then addresses it by GUID (the documented, stable key) is told the mapping does not exist, and any client reading SessionMapping.IdentityId crashes on an always-present modeled member.
Fix: the emr crate's create_studio_session_mapping / get_studio_session_mapping / list_studio_session_mappings handlers and the session-mapping state — resolve the supplied identity against the Identity Store at create time, store id and name together, and serialize IdentityId on every SessionMapping.
Traces: emr.studio-session-mapping-unicode-identity-preserved, emr.studio-session-mapping-unicode-identity-name-preserved, emr.studio-session-mapping-missing-identity-rejected, emr.e2e-studio-session-start-execute-terminate-flow.

=== Cause 5 of 8: CreateStudioSessionMapping accepts an identity that does not exist in IAM Identity Center ===
AWS: Smithy CreateStudioSessionMapping models exactly two errors, InvalidRequestException (client, HTTP 400) and InternalServerError (500). EMR validates the identity against the Studio's Identity Center instance, so a random GUID that resolves to no user is refused 400 InvalidRequestException and no mapping is created.
Emulator: the call succeeds (HTTP 200, empty body) and a mapping for the nonexistent identity is created.
Why: a client gets a Studio mapping that grants nothing and can never be used, with no signal that the identity was wrong; a negative test of identity validation can never fail.
Fix: the emr crate's CreateStudioSessionMapping handler validation — look the IdentityId/IdentityName up in the Studio's Identity Store before storing, and refuse with InvalidRequestException when it does not resolve.
Traces: emr.studio-session-mapping-invalid-identity-rejected.

=== Cause 6 of 8: CreateStudioSessionMapping accepts a malformed SessionPolicyArn ===
AWS: Smithy CreateStudioSessionMapping.SessionPolicyArn is required (XmlStringMaxLen256) and must be an IAM policy ARN; a value that is not an ARN at all (e.g. 'not-an-arn') is refused 400 InvalidRequestException and creates nothing.
Emulator: HTTP 200 with an empty JSON body ({}), and the mapping is created with the malformed policy ARN.
Why: a typo in the session policy ARN silently produces a mapping whose session policy cannot exist, so the Studio user's permissions are not what the client asked for and the error surfaces only much later, if ever.
Fix: the emr crate's CreateStudioSessionMapping handler validation — validate SessionPolicyArn's ARN form (and that the policy resolves) before storing.
Traces: emr.studio-session-mapping-invalid-session-policy-arn-rejected.

=== Cause 7 of 8: the 'no such session mapping' refusal carries emulator prose instead of AWS's sentence and AWSEditors envelope ===
AWS: Smithy models InvalidRequestException (HTTP 400) for GetStudioSessionMapping and no not-found error, so an absent mapping is reported as InvalidRequestException whose Message is AWS's fixed sentence 'Studio session mapping does not exist.' followed by the EMR Studio backend envelope '(Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <request id>; Proxy: null)'. Fixed text: the sentence and every label in the envelope; the only variable part is the request id, which is per-run and is NOT a requirement.
Emulator: status 400 and code InvalidRequestException are right, but the Message is the emulator-authored "Studio '<studio-id>' has no session mapping for <IdentityType> '<identity-id>'." with no AWS sentence and no envelope.
Why: clients and SDK/CLI users that match on AWS's sentence (or parse the envelope for the service and request id) do not recognise the refusal.
Fix: the emr crate's Studio session-mapping refusal construction (the shared GetStudioSessionMapping / DeleteStudioSessionMapping not-found path) — emit AWS's sentence plus the AWSEditors envelope.
Note: the open issue 20261102T094623Z-emr-studio-notebook-refusals-drop-aws-s-sentence.md describes this same family for OTHER check keys; it does not carry this check's replay key, so this check must be re-replayed to confirm the fix.
Traces: emr.studio-session-mapping-delete-then-get-not-found.

=== Cause 8 of 8: Spark/S3 step scenarios never settle — the checks exhaust the suite budget with no terminal step state ever observed ===
AWS: recorded behavior only. A step added to a WAITING cluster is picked up, runs and reaches a terminal state (COMPLETED or FAILED) within the scenario's deadline, and ListSteps reports that terminal state while the step's output lands in S3.
Emulator: the checks' polls for a terminal step state never satisfy and each check ends with 'Check timed out after 60.0s'; no step is ever observed leaving its non-terminal state. Hypothesis: steps are accepted and recorded but never executed, so no step reaches a terminal state and no output is written (the same family as open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md, which does not carry these check keys).
Why: a client that submits a step and waits on its terminal state waits forever, and any assertion about the step's effect (written S3 objects, Hive-layout partitions, exact numeric results) can never be made.
Fix: the emr crate's step-execution lifecycle (the AddJobFlowSteps / ListSteps state machine and whatever drives step state forward) and the step's data-plane effects.
Traces: emr.spark-step-partitioned-write-lands-hive-layout-in-s3, emr.spark-step-with-a-missing-input-path-fails-and-writes-no-output, emr.spark-bigint-aggregate-beyond-float64-precision-is-exact.

## Evidence
Behavioral suite replay against the emulator built from this checkout (49/275 passed; emr 46/216).

Cause 1 — e.g. check emr.e2e-cluster-create-step-execute-terminate-flow [error]:
  expected: CreateVpcEndpoint(VpcId, ServiceName='com.amazonaws.<region>.s3', VpcEndpointType='Gateway', RouteTableIds=[<rtb>]) -> HTTP 200, VpcEndpoint.RouteTableIds=[<rtb>].
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator.
  (byte-identical detail on ~130 other emr checks in this run.)

Cause 2 — check emr.deploy-emr-studio-via-cloudformation-session-mapping-usable [error]:
  expected: CreateStack with a template declaring 'Type: AWS::EMR::Studio' and 'Type: AWS::EMR::StudioSessionMapping' -> HTTP 200 with a StackId.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EMR::Studio, AWS::EMR::StudioSessionMapping].

Cause 3 — check emr.emr-block-public-access-rejects-a-cluster-with-an-open-security-group [mismatch]:
  expected: with BlockPublicSecurityGroupRules=true and port 1234 not in PermittedPublicSecurityGroupRuleRanges, a cluster whose security group allows 0.0.0.0/0:1234 settles TERMINATED_WITH_ERRORS with a StateChangeReason.
  actual:   AssertionError: a cluster whose security group is open on a non-permitted port must be refused, landing in TERMINATED_WITH_ERRORS; got 'WAITING' (reason None).

Cause 4 — check emr.studio-session-mapping-unicode-identity-preserved [error] and emr.studio-session-mapping-missing-identity-rejected [error] and emr.e2e-studio-session-start-execute-terminate-flow [error]:
  expected: GetStudioSessionMapping(...)['SessionMapping']['IdentityId'] == the Identity Center GUID.
  actual:   KeyError: 'IdentityId' (the member is absent from the response).
  check emr.studio-session-mapping-unicode-identity-name-preserved [error]: a mapping created with IdentityType=GROUP + IdentityName=<unicode name> is then read by IdentityId=<group GUID>:
  actual:   botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetStudioSessionMapping operation: Studio 'es-0000000000001' has no session mapping for GROUP 'd63c1ed7-48cc-4789-8ea7-b52fa361923a'. (ids are per-run values, not requirements).

Cause 5 — check emr.studio-session-mapping-invalid-identity-rejected [mismatch]:
  expected: CreateStudioSessionMapping(StudioId, IdentityType='USER', IdentityId=<random GUID>, SessionPolicyArn=<valid>) -> 400 InvalidRequestException, no mapping created.
  actual:   AssertionError: expected the call to be rejected, but it succeeded.

Cause 6 — check emr.studio-session-mapping-invalid-session-policy-arn-rejected [mismatch]:
  expected: CreateStudioSessionMapping(..., SessionPolicyArn='not-an-arn') -> 400 InvalidRequestException.
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}} (an empty JSON body).

Cause 7 — check emr.studio-session-mapping-delete-then-get-not-found [mismatch]:
  expected message: "Studio session mapping does not exist. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <request id>; Proxy: null)".
  actual message:   "Studio 'es-0000000000001' has no session mapping for USER 'e230fd66-5e08-4193-9e4b-573e8fc70881'." (status 400 and code InvalidRequestException are correct; the studio id and GUID are per-run values).

Cause 8 — checks emr.spark-step-partitioned-write-lands-hive-layout-in-s3, emr.spark-step-with-a-missing-input-path-fails-and-writes-no-output, emr.spark-bigint-aggregate-beyond-float64-precision-is-exact [mismatch]:
  expected: the submitted step(s) reach a terminal state (COMPLETED/FAILED) and their S3 effects are observable.
  actual:   Error: Check timed out after 60.0s — no terminal step state was ever observed.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay the check ids in trace_ids.
3. Cause 1: CreateVpc, CreateSubnet, CreateRouteTable, then CreateVpcEndpoint(VpcId=<vpc>, VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[<rtb>]). Expect 200 with VpcEndpoint.RouteTableIds=[<rtb>]; observe 501 NotImplemented.
4. Cause 2: CreateStack with a template whose Resources declare Type: AWS::EMR::Studio and Type: AWS::EMR::StudioSessionMapping. Expect 200 + StackId; observe 400 ValidationError 'Unrecognized resource types'.
5. Cause 3: PutBlockPublicAccessConfiguration(BlockPublicSecurityGroupRules=true) with no exception for port 1234; create a security group with an ingress rule 0.0.0.0/0 on tcp/1234 and RunJobFlow a cluster using it as EmrManagedMasterSecurityGroup. Poll DescribeCluster: expect TERMINATED_WITH_ERRORS; observe WAITING.
6. Causes 4-7: CreateStudio(AuthMode='SSO', IdcInstanceArn=<idc>), then (4) CreateStudioSessionMapping(IdentityType='GROUP', IdentityName=<group name>, SessionPolicyArn=<valid>) and GetStudioSessionMapping by IdentityName (expect SessionMapping.IdentityId present) and by IdentityId (expect the same mapping); (5) CreateStudioSessionMapping with IdentityId=<random GUID> (expect 400 InvalidRequestException); (6) CreateStudioSessionMapping with SessionPolicyArn='not-an-arn' (expect 400 InvalidRequestException); (7) DeleteStudioSessionMapping then GetStudioSessionMapping (expect 400 InvalidRequestException with message 'Studio session mapping does not exist. (Service: AWSEditors; ...)').
7. Cause 8: RunJobFlow a cluster, wait for WAITING, AddJobFlowSteps with a spark-submit step, then poll ListSteps for a terminal state; observe that no step ever reaches COMPLETED/FAILED within the budget.
