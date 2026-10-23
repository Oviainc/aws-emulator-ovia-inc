Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: grafana.deploy-workspace-via-cloudformation-persists-update grafana.grafana-http-api-native-round-trip
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana|CreateStack|ValidationError
Issue-Key: grafana.grafana-http-api-native-round-trip|_poll_grafana_api#9|Timeout|time.sleep(interval_s)
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Grafana: 2 behavioral divergences (CloudFormation rejects AWS::Grafana::Workspace as an unrecognized resource type; workspace native Grafana HTTP API unserved for service-account tokens)

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack, CreateWorkspaceServiceAccountToken

## Description
Two distinct, net-new root causes in the Amazon Managed Grafana emulator, both grounded in failing behavioral checks of the `grafana` service. They are independent and BOTH must be closed before this issue is submitted.

=== Cause 1 of 2: CloudFormation does not know the AWS::Grafana::Workspace resource type at all — CreateStack rejects the template as a format error ===
Failing check ids: grafana.deploy-workspace-via-cloudformation-persists-update
  AWS: `AWS::Grafana::Workspace` is a real CloudFormation resource type for this service, so a template declaring it is accepted by CreateStack and provisioned through the Grafana control plane. Smithy (services/service-com-amazonaws-grafana-awsgrafanacontrolplane/smithy-model/com-amazonaws-grafana-awsgrafanacontrolplane.json) models the control-plane operations the provider must drive: CreateWorkspace, DescribeWorkspace (GET /workspaces/{workspaceId}, output workspace: WorkspaceDescription required, errors ResourceNotFoundException 404 / ValidationException 400), UpdateWorkspace, DeleteWorkspace; WorkspaceId carries pattern ^g-[0-9a-f]{10}$. The CFN side is recorded behavior only: the recording shows CreateStack accepting a template whose single resource is of Type AWS::Grafana::Workspace (Properties AccountAccessType, AuthenticationProviders, PermissionType, RoleArn, Name, Description), the stack reaching StackStatus CREATE_COMPLETE with that resource at ResourceStatus CREATE_COMPLETE, the underlying workspace reaching status ACTIVE with the CFN-declared Description visible via DescribeWorkspace, and {"Ref": "<logical id>"} / DescribeStackResources PhysicalResourceId both being the minted workspace id (a ^g-[0-9a-f]{10}$ value usable directly against DescribeWorkspace). A later direct UpdateWorkspace still persists (the control plane is not CFN-exclusive), and DeleteStack tears the workspace down so a subsequent DescribeWorkspace/UpdateWorkspace on that id is refused with ResourceNotFoundException (404).
  Emulator: CreateStack never gets as far as provisioning anything — it refuses the template outright with error code `ValidationError` and the message example "Template format error: Unrecognized resource types: [AWS::Grafana::Workspace]". The type is simply absent from the emulator's CFN resource-type registry, so no stack, no stack resource and no workspace are ever created, and the whole deploy path (CREATE_COMPLETE, Ref/PhysicalResourceId, post-create direct update, stack-delete teardown) is unreachable.
  Why: any infrastructure-as-code workflow that provisions Grafana workspaces — the normal way they are created — fails at template validation against the emulator while working against AWS. A client cannot deploy, cannot read the workspace id back out of its stack, and cannot exercise stack-driven teardown; CDK/CFN-based test suites abort on the very first call rather than reporting a fidelity difference.
  Fix: the CloudFormation resource provider surface for this service — register `AWS::Grafana::Workspace` as a known resource type and implement its provider so create drives the Grafana control plane (CreateWorkspace with the template's AccountAccessType/AuthenticationProviders/PermissionType/RoleArn/Name/Description), records the minted `g-` workspace id as the resource's PhysicalResourceId and Ref value, and stack delete removes that workspace. Note the related already-filed issue 20261022T092712Z-grafana-workspace-control-plane-6-behavioral-divergences-delete.md, whose Cause 6 describes this same check failing LATER, with the stack reaching CREATE_COMPLETE but Ref resolving to the workspace NAME; that is a different, downstream signature and does not cover today's up-front template rejection, which must be fixed first for that cause to even be observable.
  Traces: grafana.deploy-workspace-via-cloudformation-persists-update (errors on the very first CreateStack).

=== Cause 2 of 2: the workspace's native Grafana HTTP API is not served, so a service-account token cannot authenticate against the announced endpoint ===
Failing check ids: grafana.grafana-http-api-native-round-trip
  AWS: Smithy models the control plane that mints the credential — CreateWorkspaceServiceAccount (POST /workspaces/{workspaceId}/serviceaccounts, 200, output id/name/grafanaRole/workspaceId all required) and CreateWorkspaceServiceAccountToken (POST /workspaces/{workspaceId}/serviceaccounts/{serviceAccountId}/tokens, 200, required output serviceAccountToken: ServiceAccountTokenSummaryWithKey carrying the bearer `key`; secondsToLive @range 1..2592000 @required) — and DescribeWorkspace returns WorkspaceDescription, which carries the workspace `endpoint`. The data-plane contract itself is recorded behavior only: the recording shows the `endpoint` reported for an ACTIVE workspace serving Grafana's own REST API, where `GET <endpoint>/api/org` with header `Authorization: Bearer <key from CreateWorkspaceServiceAccountToken>` answers HTTP 200 with a JSON object containing `id` and `name`.
  Emulator: the control-plane calls succeed and a token key is returned, but no request to the announced workspace endpoint ever receives an HTTP answer. The check's retry loop over `GET <endpoint>/api/org` with that bearer key never observes 200 and keeps retrying on transport errors until the harness kills it: "Check timed out after 60.0s". The endpoint is announced by DescribeWorkspace but is not routed on an address the client is reached on, so the data plane is absent rather than merely incomplete.
  Why: a client that does the only thing a workspace service-account token is for — calling the workspace's Grafana HTTP API with it — hangs instead of getting a response, and cannot distinguish a broken token from an unreachable workspace. Dashboard/datasource provisioning code that mints a service account, mints a token, then drives Grafana's REST API blocks until its own timeout against the emulator while working against AWS. It also means the emulator cannot show that a token actually grants anything, so the whole service-account authorization story is unverifiable.
  Fix: the workspace data-plane surface for this service — the component that serves the native Grafana HTTP API on the endpoint DescribeWorkspace reports, plus bearer-token authentication against the service-account token records persisted by CreateWorkspaceServiceAccountToken (services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace_service_account_token.rs and the token store it writes). At minimum: route the announced endpoint on the address the emulator is reached on, accept `Authorization: Bearer <key>` for live service-account tokens, and answer 200 with a JSON org object for GET /api/org. Note the related already-filed issue 20261023T031911Z-grafana-2-behavioral-divergences-workspace-native-http-api.md, whose Cause 1 covers the same unserved endpoint reached with a CreateWorkspaceApiKey key; this cause is the service-account-token credential path (a different minting operation and a different key store) and is not carried by that issue's replay key, so it is filed here and should be closed together with it if the endpoint is implemented once.
  Traces: grafana.grafana-http-api-native-round-trip (harness timeout at the first poll of <endpoint>/api/org).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): grafana 9/19 passed (47%), 10 failed. Eight of those failures are already filed (seven OWNED by existing grafana issues, one blocked by the unimplemented DeleteWorkspaceServiceAccountToken stub) and are excluded. The two causes above:

Cause 1 — grafana.deploy-workspace-via-cloudformation-persists-update [error]
  Request: cloudformation.CreateStack(StackName=<unique>, TemplateBody={"AWSTemplateFormatVersion":"2010-09-09","Resources":{"GrafanaWorkspace":{"Type":"AWS::Grafana::Workspace","Properties":{"AccountAccessType":"CURRENT_ACCOUNT","AuthenticationProviders":["SAML"],"PermissionType":"SERVICE_MANAGED","RoleArn":<role arn>,"Name":<name>,"Description":"behav-cfn-initial-description"}}},"Outputs":{"WorkspaceId":{"Value":{"Ref":"GrafanaWorkspace"}}}}, OnFailure="DELETE")
  Expected (AWS, recorded): CreateStack accepted; stack reaches CREATE_COMPLETE; DescribeStackResources(LogicalResourceId="GrafanaWorkspace").PhysicalResourceId == the minted workspace id (^g-[0-9a-f]{10}$) with ResourceStatus CREATE_COMPLETE; DescribeWorkspace on it shows status ACTIVE and description "behav-cfn-initial-description"; a direct UpdateWorkspace persists; after DeleteStack, DescribeWorkspace and UpdateWorkspace on that id answer ResourceNotFoundException (404).
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Grafana::Workspace]. Nothing is created; every later assertion is unreachable.

Cause 2 — grafana.grafana-http-api-native-round-trip [mismatch]
  Requests: CreateWorkspace(accountAccessType=CURRENT_ACCOUNT, permissionType=SERVICE_MANAGED, authenticationProviders=[SAML], workspaceRoleArn=<role>) -> poll DescribeWorkspace to ACTIVE, read workspace.endpoint -> CreateWorkspaceServiceAccount(name=<unique>, grafanaRole="ADMIN") -> CreateWorkspaceServiceAccountToken(serviceAccountId=<id>, name=<unique>, secondsToLive=3600) -> GET <endpoint>/api/org with header Authorization: Bearer <serviceAccountToken.key>.
  Expected (AWS, recorded): HTTP 200 with a JSON body containing `id` and `name`.
  Actual (emulator): no HTTP answer is ever obtained from the announced endpoint; the poll retries on transport errors and the run is terminated: "Error: Check timed out after 60.0s".

## Steps to Reproduce
Boot the emulator and point botocore `grafana` and `cloudformation` clients at it.

Cause 1 (grafana.deploy-workspace-via-cloudformation-persists-update):
1. iam.CreateRole with a trust policy for grafana.amazonaws.com; keep the role ARN.
2. cloudformation.CreateStack(StackName=<unique>, OnFailure="DELETE", TemplateBody=a template with one resource of Type "AWS::Grafana::Workspace" (Properties AccountAccessType=CURRENT_ACCOUNT, AuthenticationProviders=["SAML"], PermissionType=SERVICE_MANAGED, RoleArn=<role arn>, Name=<unique>, Description="behav-cfn-initial-description") and Outputs.WorkspaceId = {"Ref": "GrafanaWorkspace"}).
   Expected: the call is accepted. Observed: ValidationError "Template format error: Unrecognized resource types: [AWS::Grafana::Workspace]".
3. (Blocked by step 2) Poll DescribeStacks until CREATE_COMPLETE; read Outputs.WorkspaceId and DescribeStackResources(LogicalResourceId="GrafanaWorkspace").PhysicalResourceId — both must be the ^g-[0-9a-f]{10}$ workspace id; DescribeWorkspace on it must show ACTIVE with the declared description.
4. (Blocked) grafana.UpdateWorkspace(workspaceId=<that id>, workspaceDescription="behav-direct-update") and confirm it persists.
5. (Blocked) DeleteStack and wait for DELETE_COMPLETE, then confirm DescribeWorkspace and UpdateWorkspace on that id both answer ResourceNotFoundException (404).

Cause 2 (grafana.grafana-http-api-native-round-trip):
1. Create an IAM role trusted by grafana.amazonaws.com, then grafana.CreateWorkspace(accountAccessType=CURRENT_ACCOUNT, permissionType=SERVICE_MANAGED, authenticationProviders=["SAML"], workspaceRoleArn=<role>).
2. Poll DescribeWorkspace until status == ACTIVE and read workspace.endpoint.
3. CreateWorkspaceServiceAccount(workspaceId=<id>, name=<unique>, grafanaRole="ADMIN").
4. CreateWorkspaceServiceAccountToken(workspaceId=<id>, serviceAccountId=<id from step 3>, name=<unique>, secondsToLive=3600) and keep serviceAccountToken.key.
5. Issue GET <endpoint>/api/org with header "Authorization: Bearer <key>".
   Expected: HTTP 200 with a JSON body containing `id` and `name`. Observed: no HTTP answer at all — retries never succeed and the check is killed at the 60s harness budget.
