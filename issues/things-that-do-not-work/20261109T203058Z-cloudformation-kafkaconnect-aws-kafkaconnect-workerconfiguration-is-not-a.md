Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kafkaconnect.deploy-kafkaconnect-workerconfiguration-round-trip-and-delete cloudformation__kafkaconnect.deploy-kafkaconnect-workerconfiguration-name-too-long-rejected
Issue-Key: cloudformation__kafkaconnect|CreateStack|ValidationError
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# cloudformation__kafkaconnect: AWS::KafkaConnect::WorkerConfiguration is not a registered resource type — CreateStack refuses every worker-configuration template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::KafkaConnect::WorkerConfiguration, so CreateStack rejects the whole template at submit time and no KafkaConnect worker configuration is ever provisioned, read back, validated or deleted through CloudFormation.

AWS: AWS::KafkaConnect::WorkerConfiguration is a real, generally available CloudFormation resource type backed by the MSK Connect API. CreateStack on a template whose only resource is of that type is ACCEPTED (HTTP 200, a StackId returned) and the stack proceeds to CREATE_COMPLETE, having called KafkaConnect CreateWorkerConfiguration. Smithy (com-amazonaws-kafkaconnect-kafkaconnect): CreateWorkerConfiguration POST /v1/worker-configurations, input `name` required with @length min 1 max 128, `description` @length max 1024, `propertiesFileContent` required and @sensitive; output carries `workerConfigurationArn`, `name`, `workerConfigurationState` (ACTIVE) and `latestRevision` (revision 1, propertiesFileContent); DescribeWorkerConfiguration GET /v1/worker-configurations/{workerConfigurationArn} returns the same fields, errors BadRequestException 400 / NotFoundException 404. The resource's Ref and its WorkerConfigurationArn attribute both resolve to that ARN, and the configuration is listed by ListWorkerConfigurations under its name. Because the type is registered, AWS also enforces its property schema before provisioning: a Name longer than the modelled 128-character maximum does not produce a template-format error, it produces a resource-level property-validation failure that rolls the stack back to ROLLBACK_COMPLETE with a VALIDATION_ERROR operation event (ValidationName PROPERTY_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationPath /Resources/<LogicalId>/Properties/Name and a reason of the form `#/Name: expected maxLength: 128, actual: <actual length>`) and creates no resources.

Emulator: CreateStack is refused before any stack is created. HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::KafkaConnect::WorkerConfiguration]`. In that message the fixed text is `Template format error: Unrecognized resource types: [` ... `]` and the variable part is the list of resource-type names taken from the submitted template — here the single type AWS::KafkaConnect::WorkerConfiguration. Both failing checks die on the very first create_stack call, so nothing downstream (CREATE_COMPLETE, Ref/GetAtt outputs, DescribeWorkerConfiguration round-trip, stack-delete removing the configuration, and the over-128 Name property-validation rollback) is reachable; the oversize-Name rejection path is blocked by this same cause and cannot be observed separately until the type is registered.

Why: a client that models MSK Connect worker configurations in CloudFormation (or in CDK, which synthesizes the same type) cannot deploy at all — the stack is refused at submit time with a template-format error that tells the user their template is malformed rather than that the emulator lacks the type. Infrastructure-as-code flows that create a worker configuration and then reference its ARN from an AWS::KafkaConnect::Connector are entirely blocked, and the error is not one AWS would ever return for this template.

Fix: CloudFormation's template resource-type registry plus a new KafkaConnect worker-configuration resource handler. Register AWS::KafkaConnect::WorkerConfiguration with its property schema (Name required, length 1..128; Description length 0..1024; PropertiesFileContent required; Tags) so property validation runs and emits the PROPERTY_VALIDATION operation event and ROLLBACK_COMPLETE on a violation; wire create to KafkaConnect CreateWorkerConfiguration, read to DescribeWorkerConfiguration and delete to DeleteWorkerConfiguration against the same KafkaConnect state that ListWorkerConfigurations/DescribeWorkerConfiguration serve; set the physical resource id, Ref and the WorkerConfigurationArn attribute to the returned workerConfigurationArn.

Traces: both failing deploy checks below fail on the first CreateStack with this identical ValidationError.

## Evidence
Check cloudformation__kafkaconnect.deploy-kafkaconnect-workerconfiguration-round-trip-and-delete:
  expected: create_stack(TemplateBody={"Resources":{"Wc":{"Type":"AWS::KafkaConnect::WorkerConfiguration","Properties":{"Name":<name>,"Description":<desc>,"PropertiesFileContent":<base64 properties>}}},"Outputs":{"WorkerConfigRef":{"Value":{"Ref":"Wc"}},"WorkerConfigArn":{"Value":{"Fn::GetAtt":["Wc","WorkerConfigurationArn"]}}}}) accepted -> StackId; stack reaches CREATE_COMPLETE; both outputs equal the single ARN ListWorkerConfigurations(namePrefix=<name>) reports; DescribeWorkerConfiguration on it returns (arn, name, description, "ACTIVE", latestRevision.revision 1, propertiesFileContent); after DeleteStack the configuration is gone (NotFoundException) and no longer listed.
  actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::KafkaConnect::WorkerConfiguration]

Check cloudformation__kafkaconnect.deploy-kafkaconnect-workerconfiguration-name-too-long-rejected:
  expected: the same template with a Name of >128 characters is ACCEPTED by CreateStack, then the stack ends ROLLBACK_COMPLETE; DescribeEvents OperationEvents contains exactly one EventType=VALIDATION_ERROR row (LogicalResourceId "Wc", ValidationName "PROPERTY_VALIDATION", ValidationStatus "FAILED", ValidationFailureMode "FAIL", ValidationStatusReason "#/Name: expected maxLength: 128, actual: <len(Name)>", ValidationPath "/Resources/Wc/Properties/Name"); DescribeStackResources returns []; KafkaConnect lists nothing under that name; and the control stack with a valid Name reaches CREATE_COMPLETE with the configuration ACTIVE.
  actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::KafkaConnect::WorkerConfiguration]

No open issue covers this type; the two open kafkaconnect issues (20261029T194141Z-kafkaconnect-connector-lifecycle-never-reaches-failed-no-statedescription.md and 20261031T114752Z-kafkaconnect-updateconnector-resolves-the-connector-before-validating-currentversion-answering.md) concern the KafkaConnect connector APIs directly, not CloudFormation resource-type registration.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client("cloudformation"); submit a template whose only resource is {"Type": "AWS::KafkaConnect::WorkerConfiguration", "Properties": {"Name": "<<=128 chars>", "PropertiesFileContent": base64("key.converter=org.apache.kafka.connect.storage.StringConverter\nvalue.converter=org.apache.kafka.connect.storage.StringConverter\n")}} with Outputs Ref and Fn::GetAtt WorkerConfigurationArn:
   cfn.create_stack(StackName="kcwc-1", TemplateBody=json.dumps(template), Tags=[{"Key":"sim2real","Value":"kcwc-1"}])
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::KafkaConnect::WorkerConfiguration]" instead of a StackId. AWS accepts the call and the stack reaches CREATE_COMPLETE.
4. Repeat with Name longer than 128 characters: AWS still ACCEPTS CreateStack and the stack ends ROLLBACK_COMPLETE with a PROPERTY_VALIDATION VALIDATION_ERROR operation event on /Resources/Wc/Properties/Name; the emulator returns the same template-format ValidationError instead.
