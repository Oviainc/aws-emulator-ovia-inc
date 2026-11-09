Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codeconnections.deploy-cfn-codeconnections-connection-and-host-round-trip-and-delete cloudformation__codeconnections.cfn-codeconnections-host-invalid-provider-endpoint-rejected-and-rolled-back
Issue-Key: cloudformation__codeconnections|CreateStack|ValidationError
Issue-Commit: 74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4

# cloudformation__codeconnections: AWS::CodeConnections::Connection and AWS::CodeConnections::Host are not registered resource types — CreateStack refuses every CodeConnections template

> _Note: this issue was identified against the code base at commit `74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry in the emulator does not know AWS::CodeConnections::Connection or AWS::CodeConnections::Host, so every template that declares either is refused at template-validation time. No CodeConnections connection or host can be provisioned, read back, rolled back or deleted by a stack, even though the CodeConnections service itself answers correctly (45/45 of this run's codeconnections-only checks pass). Failing checks: cloudformation__codeconnections.deploy-cfn-codeconnections-connection-and-host-round-trip-and-delete and cloudformation__codeconnections.cfn-codeconnections-host-invalid-provider-endpoint-rejected-and-rolled-back.

AWS: CloudFormation supports both AWS::CodeConnections::Connection and AWS::CodeConnections::Host. CreateStack with such a template is accepted (returns a StackId) and the stack proceeds to CREATE_COMPLETE. Each resource's handler calls the CodeConnections API (Smithy, com-amazonaws-codeconnections-codeconnections-20231201): CreateConnection(ConnectionName required @length 1..32, ProviderType enum, optional HostArn) -> ConnectionArn (pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$); CreateHost(Name required @length 1..64, ProviderType required, ProviderEndpoint required @length 1..512, optional VpcConfiguration) -> HostArn (pattern ^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$). The connection resource's Ref and its GetAtt ConnectionArn are both the connection ARN, GetAtt ConnectionStatus is PENDING for a freshly created connection, and the connection is listed by ListConnections(ProviderTypeFilter=GitHub) with ProviderType, ConnectionStatus PENDING and OwnerAccountId = the caller's account. The host resource's Ref is the host ARN, GetAtt HostId is the ARN's trailing id segment (the part after the last '/'), GetAtt Status is PENDING, and ListHosts (output member Hosts, paginated by NextToken) lists it with Name, ProviderType and ProviderEndpoint exactly as templated (GetHost returns the same Name/Status/ProviderType/ProviderEndpoint members). After DeleteStack both are gone: GetConnection refuses with ResourceNotFoundException (httpError 404) message "No connection exists with the ARN" and GetHost refuses with ResourceNotFoundException (404) message "No host exists with the ARN: <host arn>" — the fixed text is "No connection exists with the ARN" / "No host exists with the ARN: ", the ARN is the value from the request. When ProviderEndpoint is not a URL (e.g. "not-a-url"), CodeConnections' own CreateHost refuses the handler's call and CloudFormation records the Host resource CREATE_FAILED and rolls the stack back to ROLLBACK_COMPLETE with the registry-handler reason template `Resource handler returned message: "Provider endpoint is not valid (Service: CodeConnections; Operation: CreateHost; Status Code: 400; Error Code: ValidationException; Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the fixed text is "Provider endpoint is not valid", service CodeConnections, operation CreateHost, HTTP status 400, error code ValidationException, HandlerErrorCode InvalidRequest; the request id and request token are per-run values and not part of the requirement. After the rollback the rejected host name is absent from ListHosts while a control host created with a valid endpoint is still listed. (The ValidationException wording is recorded behavior: CreateHost's modeled error list carries only LimitExceededException, so the 400/ValidationException refusal of a non-URL ProviderEndpoint comes from the recording, not the model.)

Emulator: both checks die on their first CreateStack with HTTP 400 ValidationError and message `Template format error: Unrecognized resource types: [AWS::CodeConnections::Connection, AWS::CodeConnections::Host]` (and, for the single-resource template, `Template format error: Unrecognized resource types: [AWS::CodeConnections::Host]`). No stack is created, so none of the Ref/GetAtt, list-back, delete-refusal or invalid-endpoint rollback behavior is reachable.

Why: a client cannot express CodeConnections in infrastructure-as-code against this emulator. Every CDK/CloudFormation template that creates a GitHub connection or a GitHubEnterpriseServer host fails at submit time with a template-format error, which reads to the client as "my template is malformed" rather than "this type is unsupported". Pipelines that provision their source connection in the same stack cannot be deployed at all, and the invalid-endpoint rejection contract (ROLLBACK_COMPLETE, HandlerErrorCode InvalidRequest, no host left behind) is never exercised.

Fix: CloudFormation's resource-type registry / template validation must register AWS::CodeConnections::Connection and AWS::CodeConnections::Host, and each needs a provider/handler in the CloudFormation resource-provider layer that: on Create calls CodeConnections CreateConnection / CreateHost with the templated properties, reports the returned ARN as the physical resource id (so Ref yields the ARN), exposes GetAtt ConnectionArn/ConnectionStatus for the connection and HostId/Status for the host, on Delete calls DeleteConnection / DeleteHost, and surfaces a CodeConnections 400 refusal as a CREATE_FAILED ResourceStatusReason in the registry-handler format with HandlerErrorCode InvalidRequest so the stack rolls back. CodeConnections' own CreateHost must additionally reject a ProviderEndpoint that is not a URL with the 400 ValidationException above (its input validation), which the happy path must not reject.

Traces: cloudformation__codeconnections.deploy-cfn-codeconnections-connection-and-host-round-trip-and-delete (both types in one template; Ref/GetAtt, ListConnections/ListHosts read-back, then post-delete ResourceNotFoundException on each) and cloudformation__codeconnections.cfn-codeconnections-host-invalid-provider-endpoint-rejected-and-rolled-back (Host only; valid-endpoint control plus the non-URL rollback). Both error identically at CreateStack.

## Evidence
Both failing checks error at the first CreateStack call:

  deploy-cfn-codeconnections-connection-and-host-round-trip-and-delete:
    botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeConnections::Connection, AWS::CodeConnections::Host]

  cfn-codeconnections-host-invalid-provider-endpoint-rejected-and-rolled-back:
    botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeConnections::Host]

Template submitted (round-trip check, abridged):
  Resources:
    Connection: Type AWS::CodeConnections::Connection, Properties {ConnectionName: <name>, ProviderType: GitHub}
    Host:       Type AWS::CodeConnections::Host,       Properties {Name: <name>, ProviderType: GitHubEnterpriseServer, ProviderEndpoint: https://<name>.example.com}
  Outputs: ConnectionRef {Ref Connection}, ConnectionArnAtt {GetAtt Connection.ConnectionArn}, ConnectionStatusAtt {GetAtt Connection.ConnectionStatus}, HostArn {Ref Host}, HostIdAtt {GetAtt Host.HostId}, HostStatusAtt {GetAtt Host.Status}

Expected (AWS):
  - CreateStack returns a StackId; stack reaches CREATE_COMPLETE.
  - ListConnections(ProviderTypeFilter=GitHub) lists exactly one entry named <name> with (ConnectionArn == Ref Connection, ProviderType GitHub, ConnectionStatus PENDING, OwnerAccountId == caller account); GetAtt ConnectionArn == Ref, GetAtt ConnectionStatus == "PENDING".
  - ListHosts lists exactly one entry named <name> with (HostArn == Ref Host, ProviderType GitHubEnterpriseServer, ProviderEndpoint == the templated URL, Status == GetAtt Status == "PENDING"); GetAtt HostId == the host ARN's segment after the last '/'.
  - After DeleteStack: GetConnection(ConnectionArn=<arn>) -> ResourceNotFoundException "No connection exists with the ARN"; GetHost(HostArn=<arn>) -> ResourceNotFoundException "No host exists with the ARN: <host arn>".
  - With ProviderEndpoint "not-a-url": the control stack with a valid endpoint reaches CREATE_COMPLETE and its host reads back through GetHost; the bad stack reaches ROLLBACK_COMPLETE with exactly one CREATE_FAILED row for logical id "Host" whose reason is `Resource handler returned message: "Provider endpoint is not valid (Service: CodeConnections; Operation: CreateHost; Status Code: 400; Error Code: ValidationException; Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`, and ListHosts contains the control host but not the rejected name.

Actual (emulator): CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [...]" for both templates. No stack, no resources, none of the above observable.

Note: no open issue covers these types. The two open codeconnections issues (20261029T175842Z-codeconnections-client-error-responses-use-the-modeled-httperror.md, 20261029T175842Z-codeconnections-refusal-messages-are-emulator-authored-instead-of.md) are wire-level refusal-status/message defects of the CodeConnections API itself and register nothing with CloudFormation; the closest CloudFormation issues cover other unregistered types (e.g. 20261109T124403Z-cloudformation-s3vectors-..., 20261109T113448Z-cloudformation-network-security-manager-...).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. cloudformation.CreateStack with TemplateBody:
   {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Connection":{"Type":"AWS::CodeConnections::Connection","Properties":{"ConnectionName":"<unique-name>","ProviderType":"GitHub"}},"Host":{"Type":"AWS::CodeConnections::Host","Properties":{"Name":"<unique-name>","ProviderType":"GitHubEnterpriseServer","ProviderEndpoint":"https://<unique-name>.example.com"}}},"Outputs":{"ConnectionRef":{"Value":{"Ref":"Connection"}},"ConnectionArnAtt":{"Value":{"Fn::GetAtt":["Connection","ConnectionArn"]}},"ConnectionStatusAtt":{"Value":{"Fn::GetAtt":["Connection","ConnectionStatus"]}},"HostArn":{"Value":{"Ref":"Host"}},"HostIdAtt":{"Value":{"Fn::GetAtt":["Host","HostId"]}},"HostStatusAtt":{"Value":{"Fn::GetAtt":["Host","Status"]}}}}
   -> observe ValidationError "Template format error: Unrecognized resource types: [AWS::CodeConnections::Connection, AWS::CodeConnections::Host]" instead of a StackId.
3. cloudformation.CreateStack with only the Host resource and ProviderEndpoint "not-a-url" -> same refusal naming [AWS::CodeConnections::Host], so the expected ROLLBACK_COMPLETE / HandlerErrorCode InvalidRequest path is unreachable.
4. Replay: cloudformation__codeconnections.deploy-cfn-codeconnections-connection-and-host-round-trip-and-delete and cloudformation__codeconnections.cfn-codeconnections-host-invalid-provider-endpoint-rejected-and-rolled-back.
