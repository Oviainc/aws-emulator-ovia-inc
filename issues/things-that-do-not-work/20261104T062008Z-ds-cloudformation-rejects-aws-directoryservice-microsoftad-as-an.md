Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.deploy-microsoft-ad-cfn-behaves-active ds.deploy-microsoft-ad-cfn-directory-behaves ds.deploy-microsoft-ad-cloudformation-active ds.deploy-microsoft-ad-directory-behaves ds.deploy-microsoftad-cfn-ldaps-behaves
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|CreateStack|ValidationError
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ds: CloudFormation rejects AWS::DirectoryService::MicrosoftAD as an unrecognized resource type, so no directory can be provisioned from a template (5 checks blocked)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation front door of this emulator does not know the AWS::DirectoryService::MicrosoftAD resource type, so every template that provisions a Managed Microsoft AD is refused at CreateStack and no directory is ever created.

AWS: CloudFormation supports AWS::DirectoryService::MicrosoftAD (properties Name, Password, VpcSettings{VpcId, SubnetIds}, plus optional ShortName, Edition, EnableSso, CreateAlias). Recorded behavior: CreateStack with such a template is ACCEPTED (HTTP 200 with a StackId), the stack settles CREATE_COMPLETE, the resource's physical id is the directory id (d-<10 hex>), and DescribeDirectories then reports that directory reaching Stage=Active — which is what these five checks go on to exercise (template-provisioned directory behaves like an API-created one, including LDAPS). A template whose resource types are genuinely unknown is what AWS refuses with ValidationException 'Template format error: Unrecognized resource types: [<type>]'; a supported type must never produce it.
Emulator: CreateStack answers HTTP 400 ValidationException 'Template format error: Unrecognized resource types: [AWS::DirectoryService::MicrosoftAD]', so all five checks die on their first call and nothing downstream is exercised.
Why: infrastructure-as-code is the normal way a Managed Microsoft AD is provisioned; against this emulator every such template fails at submit time, so no CloudFormation-driven directory estate can be stood up and the refusal message tells the user the type does not exist rather than that the emulator lacks it.
Fix: the CloudFormation resource-type registry/provisioner of this emulator — register AWS::DirectoryService::MicrosoftAD and wire its create/delete to the ds CreateMicrosoftAD/DeleteDirectory handlers, returning the directory id as the resource's physical id and exposing the attributes the type documents (e.g. Alias, DnsIpAddresses).
Traces: ds.deploy-microsoft-ad-cfn-behaves-active, ds.deploy-microsoft-ad-cfn-directory-behaves, ds.deploy-microsoft-ad-cloudformation-active, ds.deploy-microsoft-ad-directory-behaves, ds.deploy-microsoftad-cfn-ldaps-behaves.

## Evidence
Suite simparity-behavioral-aws-v1 against this checkout: Total 56/178 passed (31.5%); 95 mismatched, 27 errored. The five checks below are reported under service heading 'ds' and all five carry the SAME error detail:

  [error] ds.deploy-microsoft-ad-cfn-behaves-active
  [error] ds.deploy-microsoft-ad-cfn-directory-behaves
  [error] ds.deploy-microsoft-ad-cloudformation-active
  [error] ds.deploy-microsoft-ad-directory-behaves
  [error] ds.deploy-microsoftad-cfn-ldaps-behaves
    botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DirectoryService::MicrosoftAD]

Expected: CreateStack accepted (HTTP 200, StackId returned), the stack reaching CREATE_COMPLETE, the AWS::DirectoryService::MicrosoftAD resource's PhysicalResourceId being a d-<10 hex> directory id, and DescribeDirectories on it reporting Stage=Active (one check then enables LDAPS on it).
Actual: HTTP 400 ValidationError with the 'Unrecognized resource types' template-format error; no stack, no directory.

Model evidence: the ds Smithy model (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json) carries CreateMicrosoftAD with Name/Password/VpcSettings and DescribeDirectories/EnableLDAPS, i.e. the API surface the resource type maps onto; the resource-type coverage itself is CloudFormation's and is recorded behavior, not Smithy-modelled.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() to build, boot and replay against the emulator.
2. Submit CreateStack with a minimal template containing one resource of type AWS::DirectoryService::MicrosoftAD (Name=<unique>.example.com, Password=<compliant>, VpcSettings={VpcId: <vpc>, SubnetIds: [<two subnets in different AZs>]}).
3. Expect HTTP 200 with a StackId and the stack settling CREATE_COMPLETE; observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::DirectoryService::MicrosoftAD]'.
4. After the type is registered, DescribeStackResources should report a PhysicalResourceId of the form d-<10 hex> and DescribeDirectories on it should reach Stage=Active.
5. Re-replay the five check ids in trace_ids.
