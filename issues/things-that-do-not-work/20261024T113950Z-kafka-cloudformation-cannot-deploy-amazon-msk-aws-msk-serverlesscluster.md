Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.deploy-msk-serverless-cfn-topic-createable kafka.deploy-msk-cluster-cfn-bootstrap-brokers-reachable
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateStack|ValidationError
Issue-Key: kafka|DescribeCluster|BadRequestException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CloudFormation cannot deploy Amazon MSK: AWS::MSK::ServerlessCluster is refused as unrecognized and AWS::MSK::Cluster is declared-only with a non-ARN Ref (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
TWO distinct root causes in the CloudFormation path that deploys Amazon MSK. Both must be closed.

Cause 1 of 2 — AWS::MSK::ServerlessCluster is not in the recognized resource-type catalogue, so a template declaring it is refused at submission.
AWS: recorded behavior only for the status/code (the CloudFormation Smithy model enumerates no resource types — ResourceTypes is a list of plain strings). The CloudFormation User Guide's "AWS resource and property types reference" publishes the Amazon MSK namespace including AWS::MSK::ServerlessCluster, so CreateStack with such a template is ACCEPTED (HTTP 200 with a StackId) and the stack proceeds to CREATE_COMPLETE with the serverless cluster created in MSK.
Emulator: CreateStack is refused with HTTP 400, Error.Code 'ValidationError', message (example) 'Template format error: Unrecognized resource types: [AWS::MSK::ServerlessCluster]'. The recognized set is the static list services/service-com-amazonaws-cloudformation-cloudformation/src/state/resource_types.rs `AWS_RESOURCE_TYPES`, and it carries no AWS::MSK::ServerlessCluster entry; that file's own docs note "a type the reference publishes and this list does not carry is refused".
Why: a client cannot deploy an MSK Serverless cluster by template at all — the refusal happens before the stack exists, so there is no stack, no events and no outputs to inspect, and infrastructure-as-code flows that real AWS accepts fail closed here.
Fix: services/service-com-amazonaws-cloudformation-cloudformation/src/state/resource_types.rs — add the published Amazon MSK types to AWS_RESOURCE_TYPES (at minimum AWS::MSK::ServerlessCluster; the namespace also publishes BatchScramSecret, ClusterPolicy, Configuration, Replicator and VpcConnection), and give the type the create/delete arm in src/state/provisioner.rs (provisioner_for) plus its published Ref/GetAtt return values in src/state/template.rs so the stack really creates the cluster and resolves its ARN.
Traces: kafka.deploy-msk-serverless-cfn-topic-createable.

Cause 2 of 2 — AWS::MSK::Cluster is catalogued but declared-only: the stack reaches CREATE_COMPLETE without creating the cluster in MSK, and `Ref` resolves to the cluster NAME instead of the cluster ARN.
AWS: recorded behavior plus the type's published "Return values": for AWS::MSK::Cluster, `Ref` returns the Amazon Resource Name (ARN) of the cluster, and the cluster itself exists in MSK once the stack completes — so an output `Value: !Ref MskCluster` is a value kafka.DescribeCluster(ClusterArn=…) accepts (Smithy kafka#DescribeCluster takes a ClusterArn path label and models BadRequestException 400 for a value that is not an MSK cluster ARN).
Emulator: the stack completes, but the output resolves to the ClusterName the template declared (example: 'sim2real-behav-kafka-cfn-f015a4653252'), and the subsequent kafka.DescribeCluster is refused with HTTP 400, Error.Code 'BadRequestException' (message example: '<name> is not a valid Amazon MSK cluster ARN.'). There is no AWS::MSK::* arm in src/state/provisioner.rs and no AWS::MSK::* entry in src/state/template.rs's return values, so the resource falls through to the declared-only path documented in src/state/provisioner.rs:14-17: "nothing is created in any service, and the resource still carries the physical id and attributes CloudFormation's documented naming rules give it".
Why: a stack reports success for a cluster that does not exist, and the identifier it hands back is not usable against the MSK API. Every downstream step a client drives from a stack output (GetBootstrapBrokers, ListNodes, topic creation) fails, and the failure surfaces far from its cause.
Fix: services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs — an AWS::MSK::Cluster arm that calls kafka CreateCluster with the declared BrokerNodeGroupInfo/KafkaVersion/NumberOfBrokerNodes/Tags and answers the ClusterArn MSK minted as the resource's physical id (the same arrangement the FIS/EMR/GuardDuty arms use for a service-assigned identity), plus its published return values in src/state/template.rs and the delete arm for stack teardown.
Traces: kafka.deploy-msk-cluster-cfn-bootstrap-brokers-reachable.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 11/85 passed; kafka 11/84.

Cause 1 — kafka.deploy-msk-serverless-cfn-topic-createable:
  cfn.create_stack(StackName=…, TemplateBody=<YAML declaring AWS::EC2::VPC, AWS::EC2::Subnet x2, AWS::EC2::SecurityGroup and AWS::MSK::ServerlessCluster>)
  expected: HTTP 200 with a StackId, stack reaching CREATE_COMPLETE
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MSK::ServerlessCluster]

Cause 2 — kafka.deploy-msk-cluster-cfn-bootstrap-brokers-reachable:
  template declares MskCluster (AWS::MSK::Cluster, NumberOfBrokerNodes: 2, InstanceType kafka.t3.small, ClientSubnets [SubnetA, SubnetB]) with Outputs.ClusterArn = !Ref MskCluster; the check then calls kafka.describe_cluster(ClusterArn=<that output>)
  expected: the output is the cluster ARN and DescribeCluster returns the ACTIVE 2-broker cluster
  actual:   botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the DescribeCluster operation: sim2real-behav-kafka-cfn-f015a4653252 is not a valid Amazon MSK cluster ARN.
  (the value handed back is the ClusterName parameter, i.e. the declared-only physical id, and no cluster was created in the kafka service)

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: cfn.create_stack(StackName='s1', TemplateBody=<a template whose Resources declare AWS::MSK::ServerlessCluster with ClientAuthentication.Sasl.Iam.Enabled: true and two VpcConfigs subnets>) -> expect HTTP 200 and a stack that reaches CREATE_COMPLETE; observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::MSK::ServerlessCluster]'.
3. Cause 2: cfn.create_stack(StackName='s2', TemplateBody=<a template declaring a VPC, two subnets in distinct AZs, a security group and an AWS::MSK::Cluster, with Outputs.ClusterArn = !Ref MskCluster>), wait for CREATE_COMPLETE, read Outputs.ClusterArn -> expect an 'arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>' value; observe the bare cluster name.
4. kafka.describe_cluster(ClusterArn=<that output>) -> expect HTTP 200 with the ACTIVE cluster; observe HTTP 400 BadRequestException '… is not a valid Amazon MSK cluster ARN.' Also kafka.list_clusters() -> expect the stack's cluster; observe it was never created.
5. Re-replay both check ids in trace_ids.
