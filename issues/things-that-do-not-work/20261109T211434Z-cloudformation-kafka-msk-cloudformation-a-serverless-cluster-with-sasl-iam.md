Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete
Issue-Key: cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# MSK CloudFormation: a serverless cluster with SASL/IAM disabled deploys instead of rolling back, an unsupported KafkaVersion configuration deploys, and the configuration's Ref/GetAtt Outputs carry no OutputValue (3 causes)

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
THREE distinct root causes behind the MSK CloudFormation resource types. Each must be closed separately before this issue is submitted.

Cause 1 of 3: AWS::MSK::ServerlessCluster with SASL/IAM authentication disabled is accepted and the stack reaches CREATE_COMPLETE, where MSK refuses the create and CloudFormation rolls the stack back. (Failing check: cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled.)
AWS: Smithy CreateClusterV2 (POST /api/v2/clusters, 200; errors BadRequestException 400, ConflictException 409, ForbiddenException 403, UnauthorizedException 401) takes Serverless as a ServerlessRequest whose ClientAuthentication member (ServerlessClientAuthentication -> ServerlessSasl -> Iam.Enabled) is modelled OPTIONAL, so the model does not state the rule; the recorded behavior is that MSK requires SASL/IAM on a serverless cluster and refuses a request with Iam.Enabled=false with HTTP 400 BadRequestException whose message is the fixed text `A serverless cluster must use SASL/IAM authentication.` attributed to the invalid parameter `clientAuthentication`. Through CloudFormation, the ServerlessCluster resource therefore fails CREATE, the stack ends ROLLBACK_COMPLETE, the resource's ResourceStatusReason is the handler envelope `Resource handler returned message: "[ClientRequestToken: <token>] A serverless cluster must use SASL/IAM authentication. (Service: Kafka, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>) 'clientAuthentication'" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`, and ListClustersV2 never lists the cluster. (The token, request id and attempt count are per-run values; the fixed text, the status 400, the `clientAuthentication` attribution and HandlerErrorCode InvalidRequest are the requirement.)
Emulator: the stack reaches CREATE_COMPLETE — the serverless cluster with Sasl.Iam.Enabled=false is minted, no resource failure is recorded and no rollback occurs. Suite assertion: `AssertionError: rejected stack ended 'CREATE_COMPLETE'`.
Why: a template that AWS rejects deploys clean against the emulator, so a client ships infrastructure declaring an unauthenticated serverless MSK cluster and only discovers at real-AWS deploy time that the stack cannot exist; conversely a negative test that proves "an unauthenticated serverless cluster is refused" passes vacuously.
Fix: CreateClusterV2's serverless validation in the kafka service (the Serverless/ClientAuthentication pre-check that must refuse with 400 BadRequestException before any cluster record is created) — the CloudFormation provider then surfaces that refusal as the resource failure and rolls back. The open issue 20261104T171616Z-kafka-createclusterv2-validates-no-provisioned-cluster-input-four-requests.md covers the PROVISIONED input validations only and does not carry this check's replay key.
Traces: cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled.

Cause 2 of 3: an AWS::MSK::Configuration naming an unsupported KafkaVersion is accepted and the stack reaches CREATE_COMPLETE instead of failing with MSK's refusal. (Failing check: cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back.)
AWS: recorded behavior for the CloudFormation leg — CreateConfiguration refuses a KafkaVersions entry that is not in MSK's supported catalog with HTTP 400 BadRequestException whose message names the rejected version and lists the valid ones; the MskConfig resource therefore fails CREATE, the stack ends ROLLBACK_COMPLETE, and the resource's ResourceStatusReason is the handler envelope `Resource handler returned message: "[ClientRequestToken: <token>] Unsupported KafkaVersion [<version>]. Valid values: [<comma-separated supported versions>] (Service: Kafka, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. The fixed text is `Unsupported KafkaVersion [...]. Valid values: [...]`; the variable parts are the version taken from the request and the valid-value list taken from MSK's catalog (it must contain the versions ListKafkaVersions reports ACTIVE and must not contain the rejected one). ListConfigurations never lists the rejected configuration.
Emulator: the stack reaches CREATE_COMPLETE with KafkaVersionsList = ['99.99.x']; no resource failure, no rollback. Suite assertion: `AssertionError: rejected stack ended 'CREATE_COMPLETE'`. Root cause is that the AWS::MSK::Configuration resource is declared-only — CreateConfiguration is never called during stack CREATE, so no validation of any kind happens on the template's properties.
Why: a template CloudFormation must reject deploys green, and every negative test on MSK configuration properties passes vacuously; the stack's reported success does not mean an MSK configuration exists.
Fix: the CloudFormation provider's resource mapping for AWS::MSK::Configuration — CREATE must call the kafka service's CreateConfiguration path and propagate its 400 refusal as the resource's failure (HandlerErrorCode InvalidRequest), so the stack rolls back. The related open issues 20261104T171616Z-kafka-cloudformation-does-not-materialise-aws-msk-clusterpolicy-or.md (Cause 2) and 20261103T090515Z-kafka-createconfiguration-s-unsupported-kafkaversion-refusal-renders-the-wrong.md describe the same resource type and the direct-API refusal wording respectively, and neither carries this check's replay key.
Traces: cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back.

Cause 3 of 3: the stack Outputs built from an AWS::MSK::Configuration's Ref and GetAtt carry an OutputKey with NO OutputValue, so DescribeStacks returns a structurally incomplete Output. (Failing check: cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete.)
AWS: recorded behavior — after an AWS::MSK::Configuration stack reaches CREATE_COMPLETE, DescribeStacks returns every declared Output as an entry carrying BOTH OutputKey and OutputValue: `!Ref MskConfig` resolves to the configuration's ARN (equal to ListConfigurations' Arn and accepted by DescribeConfiguration) and `!GetAtt MskConfig.LatestRevision.CreationTime` resolves to the revision's creation time rendered as millisecond UTC `YYYY-MM-DDTHH:MM:SS.mmmZ`. An Output entry with no OutputValue is never returned.
Emulator: DescribeStacks' Outputs entries for both of those have no `OutputValue` key at all, so a client reading them raises `KeyError: 'OutputValue'` (suite frame: `return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`). This is the same declared-only resource type as cause 2: with no configuration created, Ref and GetAtt resolve to nothing and the serializer emits the entry without a value instead of either a value or no entry.
Why: every consumer of stack outputs breaks on a shape AWS never produces — SDK callers KeyError or get None, cross-stack Fn::ImportValue and pipelines that read outputs to locate the deployed resource fail, and the failure is in the response shape rather than in a reported stack status, so the stack still looks successful.
Fix: the CloudFormation provider's output resolution and DescribeStacks serializer — an Output must only be emitted with a resolved OutputValue, which requires the AWS::MSK::Configuration mapping of cause 2 to return the configuration ARN as the resource's Ref value and the revision creation time for GetAtt LatestRevision.CreationTime.
Traces: cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete.

## Evidence
All three checks are listed under the suite heading `cloudformation__kafka`; none is marked ALREADY FILED or OWNED.

Cause 1 — cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled (verdict mismatch). Template: AWS::EC2::VPC + two AZ subnets + AWS::MSK::ServerlessCluster with `ClientAuthentication: {Sasl: {Iam: {Enabled: false}}}` and VpcConfigs naming both subnets. Control stack: identical template with Enabled: true.
  Expected (AWS): the bad stack ends ROLLBACK_COMPLETE; the ServerlessCluster CREATE_FAILED ResourceStatusReason masks to message `[ClientRequestToken: <token>] A serverless cluster must use SASL/IAM authentication. (Service: Kafka, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>) 'clientAuthentication'` with HandlerErrorCode `InvalidRequest`; ListClustersV2 lists no cluster with that name.
  Actual (emulator): `AssertionError: rejected stack ended 'CREATE_COMPLETE'` (check line 233/1338 `assert status == "ROLLBACK_COMPLETE"`). No refusal is produced at all.

Cause 2 — cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back (verdict mismatch). Template: AWS::MSK::Configuration with Name=<param>, ServerProperties `auto.create.topics.enable=true`, KafkaVersionsList=[`99.99.x`].
  Expected (AWS): stack ROLLBACK_COMPLETE; MskConfig's ResourceStatusReason matches `[ClientRequestToken: <token>] Unsupported KafkaVersion [99.99.x]. Valid values: [<list>] (Service: Kafka, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` with HandlerErrorCode InvalidRequest, the list containing the version ListKafkaVersions reports ACTIVE and not containing `99.99.x`; ListConfigurations does not list the configuration.
  Actual (emulator): `AssertionError: rejected stack ended 'CREATE_COMPLETE'` (check line 232).

Cause 3 — cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete (verdict error). Template: AWS::MSK::Configuration (Name, ServerProperties `auto.create.topics.enable=true\ndefault.replication.factor=2\n`, KafkaVersionsList=[<ACTIVE version>]) with Outputs ConfigArn = !Ref MskConfig and ConfigCreationTime = !GetAtt MskConfig.LatestRevision.CreationTime.
  Expected (AWS): DescribeStacks returns both Outputs with an OutputValue — ConfigArn equal to ListConfigurations' Arn and accepted by DescribeConfiguration (State ACTIVE, KafkaVersions [<version>], LatestRevision.Revision 1), ConfigCreationTime equal to that revision's CreationTime in `YYYY-MM-DDTHH:MM:SS.mmmZ`.
  Actual (emulator): `KeyError: 'OutputValue'` raised at `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` — the Output entries exist but carry no value.

Smithy cited: CreateClusterV2 POST /api/v2/clusters code 200, errors BadRequestException/400 (plus Conflict/Forbidden/Unauthorized/TooManyRequests); ServerlessRequest members VpcConfigs (required) and ClientAuthentication -> ServerlessClientAuthentication.Sasl -> ServerlessSasl.Iam.Enabled (all optional in the model, hence the rule is recorded behavior); DeleteConfiguration/DescribeConfiguration on /v1/configurations/{Arn}.

## Steps to Reproduce
Cause 1:
1. Boot the emulator. CreateStack with a template containing an AWS::EC2::VPC, two subnets in distinct AZs, and an AWS::MSK::ServerlessCluster whose ClientAuthentication.Sasl.Iam.Enabled is false and whose VpcConfigs name both subnets.
2. Poll DescribeStacks until a terminal status. Assert ROLLBACK_COMPLETE (today: CREATE_COMPLETE).
3. Read the ServerlessCluster's CREATE_FAILED event and assert its ResourceStatusReason is the resource-handler envelope carrying message `A serverless cluster must use SASL/IAM authentication.` with Status Code 400, the `'clientAuthentication'` attribution and HandlerErrorCode InvalidRequest.
4. Assert ListClustersV2 with ClusterNameFilter=<the name> lists nothing.
5. Deploy the identical template with Enabled: true and assert CREATE_COMPLETE and DescribeClusterV2 Serverless.ClientAuthentication.Sasl.Iam.Enabled == true.
6. Replay cloudformation__kafka.deploy-cfn-msk-serverless-cluster-rejects-iam-auth-disabled.

Cause 2:
1. CreateStack with a template containing one AWS::MSK::Configuration whose KafkaVersionsList is ['99.99.x'].
2. Poll DescribeStacks to a terminal status and assert ROLLBACK_COMPLETE (today: CREATE_COMPLETE), with the MskConfig failure reason carrying `Unsupported KafkaVersion [99.99.x]. Valid values: [...]`, Status Code 400 and HandlerErrorCode InvalidRequest.
3. Assert ListConfigurations does not list the configuration name.
4. Deploy the same template with a version ListKafkaVersions reports ACTIVE and assert CREATE_COMPLETE plus DescribeConfiguration State ACTIVE / KafkaVersions [<that version>].
5. Replay cloudformation__kafka.deploy-msk-configuration-invalid-kafka-version-rejected-and-rolled-back.

Cause 3:
1. CreateStack with a template containing one AWS::MSK::Configuration and Outputs ConfigArn: !Ref MskConfig and ConfigCreationTime: !GetAtt MskConfig.LatestRevision.CreationTime; wait for CREATE_COMPLETE.
2. Call DescribeStacks and assert every Outputs entry carries both OutputKey and OutputValue (today ConfigArn and ConfigCreationTime have no OutputValue).
3. Assert ConfigArn equals ListConfigurations' Arn for that name and that DescribeConfiguration on it returns State ACTIVE, KafkaVersions [<version>], LatestRevision.Revision 1, and that ConfigCreationTime equals that revision's CreationTime rendered as YYYY-MM-DDTHH:MM:SS.mmmZ.
4. DescribeConfigurationRevision(Arn, Revision=1) and assert ServerProperties equals the template's text; delete the stack and assert DescribeConfiguration then answers BadRequestException `Configuration ARN does not exist.`
5. Replay cloudformation__kafka.deploy-msk-configuration-round-trip-and-delete.
