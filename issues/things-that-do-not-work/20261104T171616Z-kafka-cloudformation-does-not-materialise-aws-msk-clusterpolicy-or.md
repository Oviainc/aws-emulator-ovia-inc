Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.deploy-cfn-cluster-policy-attached-enforced kafka.deploy-msk-configuration-cfn-applied-to-cluster
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|GetClusterPolicy|NotFoundException
Issue-Key: kafka|DescribeConfiguration|BadRequestException
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# CloudFormation does not materialise AWS::MSK::ClusterPolicy or AWS::MSK::Configuration: the stack reports success but GetClusterPolicy answers 404 and DescribeConfiguration answers 400 (2 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetClusterPolicy

## Description
TWO distinct root causes: two MSK CloudFormation resource types are declared-only, so a stack that CloudFormation reports as created leaves no MSK resource behind. Each resource type is a separate provider mapping and must be closed separately; both must be closed before this issue is submitted. The resolved issue 20261024T113950Z-kafka-cloudformation-cannot-deploy-amazon-msk-aws-msk-serverlesscluster.md fixed AWS::MSK::ServerlessCluster and AWS::MSK::Cluster; these two sibling types are still not provisioned and neither check id appears in it.

Cause 1 of 2: AWS::MSK::ClusterPolicy is not applied, so the deployed cluster has no resource policy.
AWS: a stack containing AWS::MSK::Cluster plus AWS::MSK::ClusterPolicy (Properties ClusterArn + Policy) attaches that policy to the cluster during CREATE. After the stack reaches CREATE_COMPLETE, GetClusterPolicy on the cluster returns HTTP 200 with the policy document and a CurrentVersion, with the template's statement content (Sid, Effect, Principal, Action, Resource) intact. A PutClusterPolicy applied afterwards to the same cluster produces the identically-shaped versioned result.
Emulator: GetClusterPolicy on the stack's cluster answers 404 NotFoundException 'The requested resource doesn\u2019t exist.' — the ClusterPolicy resource in the template produced no policy, even though the stack create itself completed.
Why: the stack is a lie: infrastructure that grants cross-account access through AWS::MSK::ClusterPolicy deploys 'successfully' with no policy attached, so every access decision that depends on it differs from AWS, and a deploy-then-verify pipeline that reads GetClusterPolicy fails at the verification step or (worse) silently proceeds with no grant.
Fix: the CloudFormation provider's resource mapping for AWS::MSK::ClusterPolicy — on CREATE it must call the kafka service's PutClusterPolicy path for the referenced ClusterArn (and remove it on DELETE).
Traces: kafka.deploy-cfn-cluster-policy-attached-enforced.

Cause 2 of 2: AWS::MSK::Configuration is not created, so its Ref does not resolve to a configuration the service knows.
AWS: a stack containing AWS::MSK::Configuration (Name, ServerProperties, KafkaVersionsList) creates an MSK configuration; !Ref on it yields the configuration's ARN, DescribeConfiguration on that ARN returns 200 with the configuration and its latest revision, and a cluster in the same stack that references it through BrokerNodeGroupInfo/ConfigurationInfo reports that ConfigurationArn and Revision in DescribeClusterV2's Provisioned.CurrentBrokerSoftwareInfo.
Emulator: DescribeConfiguration on the value the stack output for the configuration answers 400 BadRequestException 'Configuration ARN does not exist.' — the configuration was never registered (or the Ref is not its ARN), so the cluster cannot be shown to be using it.
Why: a client deploys a cluster it believes is running its own broker configuration and gets the service default; the cluster's CurrentBrokerSoftwareInfo cannot be reconciled with the template, and configuration-drift checks keyed on the stack's ConfigArn break.
Fix: the CloudFormation provider's resource mapping for AWS::MSK::Configuration — CREATE must call CreateConfiguration and return its ARN as the resource's Ref value, so a sibling cluster's ConfigurationInfo resolves.
Traces: kafka.deploy-msk-configuration-cfn-applied-to-cluster.

## Evidence
Both checks are kafka behavioral checks with verdict error; neither is marked ALREADY FILED or OWNED.

Cause 1 — kafka.deploy-cfn-cluster-policy-attached-enforced: template with MskCluster (AWS::MSK::Cluster, 2 brokers, kafka.t3.small) and MskClusterPolicy (AWS::MSK::ClusterPolicy with ClusterArn: !Ref MskCluster and a statement Sid AllowSameAccountDescribe / Effect Allow / Principal arn:${AWS::Partition}:iam::${AWS::AccountId}:root / Action kafka:DescribeClusterV2).
  Expected: after stack create, GetClusterPolicy(ClusterArn=<stack output>) returns 200 with that policy and a CurrentVersion.
  Actual: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetClusterPolicy operation: The requested resource doesn\u2019t exist.

Cause 2 — kafka.deploy-msk-configuration-cfn-applied-to-cluster: template with MskConfig (AWS::MSK::Configuration, Name ${AWS::StackName}-config, ServerProperties 'auto.create.topics.enable=true\\ndefault.replication.factor=2\\n', KafkaVersionsList [<version>]) and MSKCluster referencing it via ConfigurationInfo {Arn: !Ref MskConfig, Revision: 1}; outputs ClusterArn and ConfigArn.
  Expected: DescribeConfiguration(Arn=<ConfigArn output>) returns 200, and DescribeClusterV2's Provisioned.CurrentBrokerSoftwareInfo ConfigurationArn/Revision match that configuration.
  Actual: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the DescribeConfiguration operation: Configuration ARN does not exist.

## Steps to Reproduce
1. Boot the emulator; discover two distinct-AZ subnets and a security group.
2. Cause 1: CreateStack with a template containing AWS::MSK::Cluster and AWS::MSK::ClusterPolicy (ClusterArn: !Ref <cluster>, Policy with one Allow statement for kafka:DescribeClusterV2), wait for CREATE_COMPLETE, read the ClusterArn output, then call GetClusterPolicy on it. Assert 200 with the template's statement content and a CurrentVersion (not 404). Then PutClusterPolicy a different document on the same cluster and assert a versioned 200.
3. Cause 2: CreateStack with a template containing AWS::MSK::Configuration and an AWS::MSK::Cluster whose ConfigurationInfo references !Ref of it, wait for CREATE_COMPLETE, read the ConfigArn and ClusterArn outputs, then call DescribeConfiguration(Arn=<ConfigArn>). Assert 200 (not 400 'Configuration ARN does not exist.') and assert DescribeClusterV2's Provisioned.CurrentBrokerSoftwareInfo ConfigurationArn/Revision equal the stack's configuration ARN and revision 1.
4. Replay kafka.deploy-cfn-cluster-policy-attached-enforced and kafka.deploy-msk-configuration-cfn-applied-to-cluster.
