Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle
Issue-Key: mq|DescribeBroker|NotFoundException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CloudFormation AWS::AmazonMQ::Broker: Ref/stack output yields the broker NAME, not a broker id, so mq DescribeBroker 404s on a CREATE_COMPLETE stack's BrokerId

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause.

Cause 1 of 1: a stack declaring AWS::AmazonMQ::Broker reaches CREATE_COMPLETE, but the value CloudFormation returns for { "Ref": "<broker logical id>" } is the broker's NAME (the BrokerName property), not the broker id Amazon MQ addresses the broker by. Every downstream mq call made with that value fails.

AWS: AWS documents that Ref on AWS::AmazonMQ::Broker returns the Amazon MQ broker ID (the `b-<uuid>` form), and Fn::GetAtt Arn returns the broker ARN; the broker id is what mq operations take. Smithy (com-amazonaws-mq-mq) models DescribeBroker as GET /v1/brokers/{BrokerId} with required httpLabel BrokerId and errors BadRequestException (400), ForbiddenException (403), NotFoundException (404), InternalServerErrorException (500); its output's BrokerId/BrokerName are distinct members (jsonName brokerId / brokerName), i.e. the id is not the name. So a stack output whose Value is { "Ref": "RabbitMqBroker" } must be an id that DescribeBroker accepts, and a stack that reports CREATE_COMPLETE must have a broker that ListBrokers returns and DescribeBroker resolves. Hypothesis: the CFN resource provider assigns the AWS::AmazonMQ::Broker resource's PhysicalResourceId from the BrokerName property (or synthesizes it) rather than from a broker actually created in Amazon MQ, the same shape as the already-open issues 20261023T125319Z-apigatewayv2-... (Ref of AWS::ApiGatewayV2::Api yields the API name), 20261024T043150Z-cloudformation-step-functions-lambda-resources-physical-resource-id... and 20261022T154846Z-ec2-emr-... (synthesized logical/physical names) — none of those names AWS::AmazonMQ::Broker, so this resource type is not covered by them.

Emulator: the stack carved from the reference RabbitMQ-consumer template (AWS::SecretsManager::Secret, AWS::AmazonMQ::Broker RABBITMQ 3.13 mq.m5.large SINGLE_INSTANCE, AWS::IAM::Role, AWS::Lambda::Function, AWS::Lambda::EventSourceMapping) reaches CREATE_COMPLETE, DescribeStacks returns the Outputs, and the BrokerId output (declared as { "Ref": "RabbitMqBroker" }) holds the value passed in as the BrokerName parameter. mq DescribeBroker(BrokerId=<that value>) then answers 404 NotFoundException ("Cannot find broker ID [...]"), which aborts the replay in phase 1 of 4 (deploy).

Why: a client cannot address a CloudFormation-deployed Amazon MQ broker at all. Everything keyed on the Ref'd id — DescribeBroker, the broker's configuration id, the broker's /aws/amazonmq/broker/<id>/... log groups, cross-stack exports and any deploy script that reads the id from stack outputs — fails or targets nothing, and the stack reporting CREATE_COMPLETE makes the estate look healthy. It also masks whether the broker exists in Amazon MQ state at all: nothing after this point in the lifecycle (mapping state, broker management plane, delete/destroy cleanup of the broker's configuration and log groups) can be exercised.

Fix: the CloudFormation resource provider for AWS::AmazonMQ::Broker — provision the broker into Amazon MQ state during stack create and set the resource's PhysicalResourceId (hence Ref, and any output or GetAtt derived from it) to the broker id that mq DescribeBroker/ListBrokers resolve, keeping Fn::GetAtt Arn the broker ARN; DeletionPolicy Delete must delete that broker on stack delete. Verify by DescribeBroker on the stack's Ref value and by ListBrokers matching exactly one broker with the template's BrokerName.

Traces: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle (error in phase 1/4 deploy, at mq.describe_broker(BrokerId=outputs["BrokerId"]) immediately after the stack reached CREATE_COMPLETE). Note: this check id carries the composite token `lambda__mq`; the diverging value is produced by CloudFormation's broker resource provider, so it is filed under `cloudformation`.

## Evidence
Check lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle, phase 1/4 (deploy).

Template (carved reference architecture, inlined in the check) declares:
  "RabbitMqBroker": { "Type": "AWS::AmazonMQ::Broker", "Properties": { "BrokerName": {"Ref": "BrokerName"}, "EngineType": "RABBITMQ", "EngineVersion": "3.13", "HostInstanceType": "mq.m5.large", "DeploymentMode": "SINGLE_INSTANCE", ... } }
  Outputs: "BrokerId": { "Value": { "Ref": "RabbitMqBroker" } }, "BrokerArn": { "Value": { "Fn::GetAtt": ["RabbitMqBroker", "Arn"] } }

Expected (real AWS): DescribeStacks Outputs.BrokerId is the broker id (b-<uuid> form) and mq DescribeBroker(BrokerId=<that value>) answers HTTP 200 with brokerId == that value and brokerName == the BrokerName parameter.

Actual (emulator): Outputs.BrokerId is the BrokerName parameter value, and the immediately following DescribeBroker fails:
  _PhaseError: [phase 1/4 deploy] NotFoundException: An error occurred (NotFoundException) when calling the DescribeBroker operation: Cannot find broker ID [sim2real-refmq-broker-90aba5f2d855]. Verify that your broker exists or check broker permissions.
(the bracketed value is exactly the generated BrokerName parameter for that run; the generated name itself is environment-specific, the divergence is that a broker NAME appears where the id is required). HTTP 404 / NotFoundException is the modeled refusal for an unknown broker id, so Amazon MQ's answer is consistent — the id handed to it is wrong.

## Steps to Reproduce
1. cloudformation CreateStack with the reference template above (Capabilities=["CAPABILITY_NAMED_IAM"], Parameters BrokerName/SecretName/FunctionName/RoleName, QueueName defaulting to testQueue).
2. Poll DescribeStacks until the stack is CREATE_COMPLETE.
3. Read the Outputs: BrokerId = { "Ref": "RabbitMqBroker" }. Observe it equals the BrokerName parameter instead of a b-<uuid> broker id.
4. Call mq DescribeBroker(BrokerId=<that output value>): the emulator answers 404 NotFoundException "Cannot find broker ID [...]". Expected: 200 with the broker's description.
5. Cross-check with mq ListBrokers (paginated): exactly one BrokerSummary with that BrokerName, whose BrokerId is what Ref should have returned.
6. Delete the stack and confirm the broker (and its service-created configuration and /aws/amazonmq/broker/<id>/ log groups) are cleaned up.
