Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__transfer.deploy-transfer-as2-server-and-agreement-round-trip-and-delete cloudformation__transfer.deploy-transfer-connector-invalid-trusted-host-key-algorithm-rejected cloudformation__transfer.deploy-transfer-profile-certificate-workflow-connector-round-trip-and-delete
Issue-Key: cloudformation__transfer.deploy-transfer-as2-server-and-agreement-round-trip-and-delete|run#16|AssertionError|assert got == want, f"server (protocols, endpoint, transports, details) {got!r} != {want!r}"
Issue-Key: cloudformation__transfer.deploy-transfer-connector-invalid-trusted-host-key-algorithm-rejected|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__transfer.deploy-transfer-profile-certificate-workflow-connector-round-trip-and-delete|run#17|AssertionError|assert got == want and want == {"Certificate": [certificate_id], "Profile": [profile_id],
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# cloudformation__transfer: 3 net-new causes — DescribeServer populates SecurityGroupIds, an invalid AWS::Transfer::Connector host key never fails the stack, and Transfer Certificate/Profile/Workflow/Connector Refs are synthesized ids with nothing created at Transfer

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct root causes in the CloudFormation/Transfer deploy surface, all grounded in replayed behavioral checks.

Cause 1 of 3: DescribeServer returns EndpointDetails.SecurityGroupIds for a VPC server.
AWS: Smithy (com.amazonaws.transfer#EndpointDetails, member SecurityGroupIds) states it explicitly — "While SecurityGroupIds appears in the response syntax for consistency with CreateServer and UpdateServer operations, this field is not populated in DescribeServer responses. Security groups are managed at the VPC endpoint level ... use the EC2 DescribeVpcEndpoints API with the VpcEndpointId returned in the response." So for an EndpointType=VPC server DescribeServer echoes VpcId and SubnetIds and returns NO SecurityGroupIds member, whatever CreateServer (or the AWS::Transfer::Server resource) was given.
Emulator: DescribeServer of the stack's AS2 VPC server returns EndpointDetails with SecurityGroupIds populated from the template's group — the replayed tuple was (['AS2'], 'VPC', ['HTTP'], (<vpc id>, [<subnet id>], [<sg id>])) where AWS gives (..., (<vpc id>, [<subnet id>], None)).
Why: a client that diffs DescribeServer against its template, or round-trips DescribeServer into UpdateServer, sees a member AWS never returns; code that reads security groups from Transfer instead of EC2 DescribeVpcEndpoints is written against emulator-only data and breaks on AWS.
Fix: the DescribeServer response representation/serializer for EndpointDetails in the transfer service — keep SecurityGroupIds in Create/UpdateServer input and in state, omit it from DescribeServer output.
Traces: cloudformation__transfer.deploy-transfer-as2-server-and-agreement-round-trip-and-delete.

Cause 2 of 3: a stack whose AWS::Transfer::Connector declares an unsupported trusted-host-key algorithm reaches CREATE_COMPLETE instead of failing the resource and rolling back.
AWS: Transfer refuses a TrustedHostKeys entry whose algorithm is unsupported (an ed25519 OpenSSH public key) with InvalidRequestException, HTTP 400, message "Invalid host key algorithm."; the CloudFormation resource handler fails the Connector resource and the stack settles ROLLBACK_COMPLETE, and the failed resource event's ResourceStatusReason is the registry-handler form: Resource handler returned message: "Invalid host key algorithm. (Service: Transfer, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest). Fixed text: "Invalid host key algorithm.", the "(Service: Transfer, Status Code: 400, Request ID: ...)" and "(SDK Attempt Count: ...)" suffixes, the "Resource handler returned message: ..." wrapper and HandlerErrorCode InvalidRequest; the request id, SDK attempt count and request token are per-run values. No connector exists for the template's Url after the rollback.
Emulator: the same stack ends CREATE_COMPLETE — the invalid host key is accepted, no resource CREATE_FAILED row and no rollback occur, so the handler reason does not exist to compare.
Why: a template with a configuration AWS refuses appears to deploy successfully; rollback-on-failure and handler-error reporting cannot be exercised, and the caller believes a connector it cannot have is live.
Fix: the AWS::Transfer::Connector resource handler in the CloudFormation engine must call Transfer CreateConnector and propagate its refusal (and Transfer's CreateConnector must validate TrustedHostKeys algorithms), with the handler-failure reason assembled in the registry-handler form.
Traces: cloudformation__transfer.deploy-transfer-connector-invalid-trusted-host-key-algorithm-rejected.

Cause 3 of 3: AWS::Transfer::Certificate, ::Profile, ::Workflow and ::Connector are stub-provisioned — Ref hands back a synthesized physical name (or an ARN) and none of the resources exists at Transfer.
AWS: Ref on AWS::Transfer::Certificate/::Profile/::Workflow/::Connector returns that resource's service id (CertificateId ^c-[0-9a-f]{17}, ProfileId ^p-[0-9a-f]{17} per Smithy CreateProfile output, WorkflowId, ConnectorId), the same value Fn::GetAtt <Resource>.<Id>Id yields, and each resource is really created at Transfer: ListCertificates, ListProfiles, ListWorkflows and ListConnectors each return exactly the one item carrying the value the template declared (the certificate's Description, the profile's As2Id, the workflow's Description, the connector's Url). Recorded behavior for the list membership.
Emulator: after CREATE_COMPLETE the Refs are CloudFormation-synthesized physical names — '<stack name>-Certificate-<hex>', '<stack name>-Profile-<hex>', '<stack name>-Connector-<hex>' — and the Workflow Ref is a workflow ARN rather than the WorkflowId, while every filtered Transfer list comes back empty: {'Certificate': [], 'Profile': [], 'Workflow': [], 'Connector': []}. Nothing was created in the Transfer data plane, so DescribeCertificate/DescribeProfile/DescribeWorkflow/DescribeConnector on the templated ids, the property read-back and the post-delete ResourceNotFoundException assertions are all unreachable. (For the Workflow leg only, the empty ListWorkflows overlaps open issue 20261105T194233Z-transfer-listworkflows-reports-an-empty-workflows-list-while.md; the synthesized Refs and the other three resources are not covered by any open issue.)
Why: an infrastructure-as-code consumer gets an identifier no Transfer API accepts and a stack that reports success while the AS2/SFTP resources it is supposed to own do not exist; every downstream Ref/GetAtt wiring (profile certificate attachment, connector secret, workflow on a server) silently points at nothing.
Fix: the Transfer resource handlers in the CloudFormation engine — Certificate/Profile/Workflow/Connector must create through the Transfer service and use the returned service id as the PhysicalResourceId and Ref value.
Traces: cloudformation__transfer.deploy-transfer-profile-certificate-workflow-connector-round-trip-and-delete.

## Evidence
Cause 1: AssertionError: server (protocols, endpoint, transports, details) (['AS2'], 'VPC', ['HTTP'], ('vpc-00000000000000080', ['subnet-00000000000000089'], ['sg-0000000000000008b'])) != (['AS2'], 'VPC', ['HTTP'], ('vpc-00000000000000080', ['subnet-00000000000000089'], None)) — expected: no SecurityGroupIds member in DescribeServer's EndpointDetails (Smithy member doc); actual: the templated security group id echoed back.

Cause 2: AssertionError: rejected stack ended 'CREATE_COMPLETE' — expected: ROLLBACK_COMPLETE with the Connector resource's CREATE_FAILED reason parsing to ('Invalid host key algorithm. (Service: Transfer, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest') and ListConnectors holding no connector for the template Url; actual: stack created successfully, no failure reason at all.

Cause 3: AssertionError: listed ids {'Certificate': [], 'Profile': [], 'Workflow': [], 'Connector': []}, Refs {'Certificate': ['sim2real-cfntr2-9e2d9d5991ae-Certificate-F2C6090A211C'], 'Profile': ['sim2real-cfntr2-9e2d9d5991ae-Profile-59A26DD81110'], 'Workflow': ['arn:aws:transfer:us-west-2:000000000000:workflow/w-55da572407500c16f'], 'Connector': ['sim2real-cfntr2-9e2d9d5991ae-Connector-E272B691E389']} — expected: each Ref equals the Transfer id (c-/p-/w-/connector id) and each independent Transfer list returns exactly that one id; actual: synthesized CFN physical names (ARN for the workflow) and four empty lists.

## Steps to Reproduce
1. Replay the behavioral suite checks listed in trace_ids against the emulator.
2. Cause 1: CreateStack with an AWS::Transfer::Server (Protocols [AS2], ProtocolDetails.As2Transports [HTTP], EndpointType VPC, EndpointDetails with VpcId/SubnetIds/SecurityGroupIds), wait for CREATE_COMPLETE, then transfer:DescribeServer the output ServerId and inspect Server.EndpointDetails — SecurityGroupIds is present; AWS omits it.
3. Cause 2: CreateStack with an AWS::Transfer::Connector whose SftpConfig.TrustedHostKeys holds an ed25519 OpenSSH public key; poll DescribeStacks — the stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and DescribeStackEvents has no CREATE_FAILED row for the Connector.
4. Cause 3: CreateStack with AWS::Transfer::Certificate, ::Profile (CertificateIds referencing it), ::Workflow (a DELETE step) and ::Connector; after CREATE_COMPLETE read the stack Outputs' Ref values and call transfer:ListCertificates/ListProfiles/ListWorkflows/ListConnectors filtered by the template's Description/As2Id/Url — Refs are '<stack>-<Logical>-<hex>' (ARN for the workflow) and all four lists are empty.
