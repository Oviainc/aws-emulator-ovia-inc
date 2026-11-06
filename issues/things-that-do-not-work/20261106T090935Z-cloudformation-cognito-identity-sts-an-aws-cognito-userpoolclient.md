Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag
Issue-Key: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag|_wait_stack#10|AssertionError|raise AssertionError(f"stack {stack} reached {status!r}, wanted {want!r}; failures: "
Issue-Commit: 00357cfa6cb2d5c5eb7266904f3309887a31f26d

# cloudformation__cognito-identity__sts: an AWS::Cognito::UserPoolClient's Ref/physical id is a hyphenated CloudFormation-style name instead of a Cognito client id, so every stack feeding it into AWS::Cognito::IdentityPool rolls back

> _Note: this issue was identified against the code base at commit `00357cfa6cb2d5c5eb7266904f3309887a31f26d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
ONE net-new root cause, grounded in the single failing behavioral check under the suite heading `cloudformation__cognito-identity__sts` that the replay report marks neither ALREADY FILED nor OWNED. Every other failure in this run is already marked OWNED by an open issue or ALREADY FILED behind an unimplemented stub.

Related open issue, named as required: 20261106T081405Z-cloudformation-cognito-identity-sts-aws-cognito-userpoolclient-s.md describes the same behaviour, but it does not carry this check's replay key (the replay report does not mark the check as OWNED by it), so this filing re-grounds the cause in the failing check id in `trace_ids`. A fixer closing one should re-replay that id for both. Disregard that issue's secondary claim about member indexing (see below).

Cause 1 of 1: the physical resource id the emulator assigns to an `AWS::Cognito::UserPoolClient` — the value `{"Ref": "UserPoolClient"}` resolves to — is the generic CloudFormation synthesized name `<stack-name>-<logical-id>-<random suffix>`, not the app-client id minted by cognito-idp.

AWS: an app client id is an opaque token of the service's own shape — Smithy com.amazonaws.cognitoidentityprovider#ClientIdType: @length min 1 max 128, @pattern `^[\w+]+$`, @sensitive; it appears as UserPoolClientType.ClientId in the CreateUserPoolClient/DescribeUserPoolClient output. Word characters and `+` only, so NO hyphens (in practice a ~26-character lowercase alphanumeric string). `Ref` on AWS::Cognito::UserPoolClient returns that client id. Feeding it to an identity pool is the modeled wiring: Smithy com.amazonaws.cognitoidentity#CognitoIdentityProvider.ClientId targets CognitoIdentityProviderClientId, @length min 1 max 128, @pattern `^[\w_]+$` (doc: "The client ID for the Amazon Cognito user pool."). Because the Ref value is a real client id, it satisfies that constraint by construction and the stack reaches CREATE_COMPLETE.

Emulator: the Ref/physical id contains hyphens, so it matches neither `^[\w+]+$` (cognito-idp's own id shape) nor `^[\w_]+$` (cognito-identity's ClientId). The emulator's CreateIdentityPool validation — correctly present — then refuses the IdentityPool resource: that resource ends CREATE_FAILED and the whole stack settles ROLLBACK_COMPLETE, with the resource status reason `Resource handler returned message: "1 validation error detected: Value '<the Ref value>' at 'cognitoIdentityProviders.1.member.clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w_]+"` and HandlerErrorCode `InvalidRequest`. In that message the fixed text (`N validation error(s) detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...`), the member path and the pattern are AWS's; the quoted value, the stack name, the hex suffix and the RequestToken are values from the request and this run's state, not requirements. The refusal itself is right — the input it is handed is wrong, so the defect is upstream of it.

Why: no template that creates a user pool client and hands its id to anything — an identity pool's CognitoIdentityProviders, a principal-tag mapping, a Lambda environment variable, an app config — can deploy against the emulator. The failure surfaces inside an unrelated resource, so a user is pointed at their IdentityPool properties rather than at the client id, and any client that stores the Ref value as a client id stores something Cognito would never issue. Every downstream Cognito federation scenario (here: a principal-tag mapping gating an S3 read by session tag) is unreachable.

Fix: the CloudFormation resource provider for AWS::Cognito::UserPoolClient — its physical-resource-id (and therefore `Ref`) must be the `ClientId` returned by the cognito-idp CreateUserPoolClient call it performs, and that id generator must itself emit a value matching `^[\w+]+$`, instead of the generic stack+logical-id+suffix physical-name generator used for resources whose physical id IS a name. No change is needed in the cognito-identity CreateIdentityPool validation, and none in the constraint-violation message: its 1-based `cognitoIdentityProviders.1.member.clientId` member path is how AWS renders list members in these messages.

Traces: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag.

## Evidence
[mismatch] cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag
Frame: check_cloudformation__cognito_identity__sts_xsvc_cognito_identity_3_cfn_principal_tag_mapping_gates_s3_read_by_session_tag.py:261 `_wait_stack(cfn, stack, "CREATE_COMPLETE", _STACK_BUDGET_S)` -> :177 `raise AssertionError(...)`.

Expected (AWS): the one-stack estate (AWS::Cognito::UserPool, AWS::Cognito::UserPoolClient, AWS::Cognito::IdentityPool with CognitoIdentityProviders=[{ClientId: {Ref: UserPoolClient}, ProviderName: {Fn::GetAtt: [UserPool, ProviderName]}}], AWS::IAM::Role, AWS::Cognito::IdentityPoolRoleAttachment, AWS::Cognito::IdentityPoolPrincipalTag, AWS::S3::Bucket + BucketPolicy) reaches CREATE_COMPLETE, because Ref UserPoolClient is a Cognito client id matching ^[\w+]+$ and therefore also the identity pool's ^[\w_]+$.

Actual (emulator): AssertionError: stack <generated-name> reached 'ROLLBACK_COMPLETE', wanted 'CREATE_COMPLETE'; failures: ['<stack>: The following resource(s) failed to create: [IdentityPool]. ', 'IdentityPool: Resource handler returned message: "1 validation error detected: Value \'<stack>-UserPoolClient-<hex suffix>\' at \'cognitoIdentityProviders.1.member.clientId\' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w_]+" (RequestToken: <per-run id>, HandlerErrorCode: InvalidRequest)'].

The divergence is the SHAPE of the Ref/physical id (it contains '-'); the concrete stack name, hex suffix, RequestToken and the per-run value quoted inside the message are not requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the check id in trace_ids.
2. Minimal manual repro: cloudformation:CreateStack with a template containing (a) an AWS::Cognito::UserPool, (b) an AWS::Cognito::UserPoolClient with UserPoolId {Ref: UserPool} and GenerateSecret false, (c) an AWS::Cognito::IdentityPool with AllowUnauthenticatedIdentities false and CognitoIdentityProviders [{ClientId: {Ref: UserPoolClient}, ProviderName: {Fn::GetAtt: [UserPool, ProviderName]}}].
3. Poll DescribeStacks: the emulator settles ROLLBACK_COMPLETE; DescribeStackEvents shows IdentityPool CREATE_FAILED with the clientId pattern violation quoted in Evidence.
4. Confirm the cause directly: DescribeStackResource(LogicalResourceId='UserPoolClient') — PhysicalResourceId contains '-' and is of the form <stack>-UserPoolClient-<suffix>; it must instead equal the ClientId returned by cognito-idp CreateUserPoolClient / DescribeUserPoolClient, which must match ^[\w+]+$.
5. After the fix the stack reaches CREATE_COMPLETE, DescribeStacks output UserPoolClientId equals that ClientId, and the identity pool lists the provider with it.
