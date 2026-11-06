Issue-Severity: HIGH
Issue-Commit: d3c953798d04d353d1a0b1fe8674c5f14bb20095

# cloudformation__cognito-identity__sts: AWS::Cognito::UserPoolClient's Ref resolves to a hyphenated CFN-style physical name, not a Cognito client id, so any stack wiring it into AWS::Cognito::IdentityPool rolls back

> _Note: this issue was identified against the code base at commit `d3c953798d04d353d1a0b1fe8674c5f14bb20095`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
ONE net-new root cause, grounded in 1 failing behavioral check under the suite heading `cloudformation__cognito-identity__sts`. The check is marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: the physical id the emulator gives an `AWS::Cognito::UserPoolClient` resource (the value `{"Ref": "UserPoolClient"}` resolves to) is a CloudFormation-style synthesized name built from the stack name, the logical id and a random suffix, instead of a Cognito user-pool client id.

AWS: a Cognito user-pool app client id is an opaque token matching the service's own `ClientIdType` shape — Smithy: com.amazonaws.cognitoidentityprovider#ClientIdType, @length min 1 max 128, @pattern `^[\w+]+$`, @sensitive — i.e. word characters and `+` only, NO hyphens (in practice a ~26-character lowercase alphanumeric string). `Ref` on AWS::Cognito::UserPoolClient returns exactly that client id. Consuming it is the documented wiring for an identity pool: Smithy com.amazonaws.cognitoidentity#CognitoIdentityProvider.ClientId targets CognitoIdentityProviderClientId, @length min 1 max 128, @pattern `^[\w_]+$` (doc: "The client ID for the Amazon Cognito user pool."). So in AWS the Ref value satisfies the identity pool's constraint by construction and the stack reaches CREATE_COMPLETE.

Emulator: `Ref` yields a name of the form `<stack-name>-<logical-id>-<hex suffix>` — the recorded instance was `sim2real-xsvc-ci3-ptag-9233e448fe-UserPoolClient-85A73959E99B`. That string contains `-`, so it matches neither cognito-identity's `^[\w_]+$` nor cognito-idp's own `^[\w+]+$`. The IdentityPool resource is therefore refused by the emulator's own constraint validation, the resource ends CREATE_FAILED and the whole stack ends `ROLLBACK_COMPLETE` with the resource status reason `Resource handler returned message: "1 validation error detected: Value '<the Ref value>' at 'cognitoIdentityProviders.1.member.clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w_]+"` and HandlerErrorCode `InvalidRequest`. (The stack name, logical id, hex suffix and RequestToken in that text are this run's generated values; the shape of the Ref value is the requirement.) Note also the member index `cognitoIdentityProviders.1.member.clientId` for a single-element list — AWS numbers that path from 0.

Why: no template that creates a user pool client and feeds its id to anything — an identity pool's CognitoIdentityProviders, a principal-tag mapping, a Lambda env var, an app config — can ever deploy against the emulator. The failure surfaces as a rollback inside an unrelated resource, so a user debugging it is pointed at their IdentityPool properties rather than at the id. Every downstream Cognito federation scenario (here: principal-tag mapping gating an S3 read by session tag) is unreachable, and a client that stores the Ref value as a client id stores something Cognito would never issue.

Fix: the CloudFormation resource provider for AWS::Cognito::UserPoolClient — its physical-resource-id / `Ref` must be the `ClientId` returned by the cognito-idp CreateUserPoolClient call it makes (and that generator must itself emit an id matching `^[\w+]+$`), rather than the generic stack+logical-id+suffix physical-name generator used for resources whose physical id IS a name.

Traces: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag.

## Evidence
[mismatch] cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag
Frame: check_...py:261 `_wait_stack(cfn, stack, "CREATE_COMPLETE", _STACK_BUDGET_S)` -> :177 raise AssertionError.

Expected (AWS): the stack (UserPool, UserPoolClient, IdentityPool with CognitoIdentityProviders=[{ClientId: {Ref: UserPoolClient}, ProviderName: {Fn::GetAtt: [UserPool, ProviderName]}}], FederatedRole, IdentityPoolRoleAttachment, IdentityPoolPrincipalTag) reaches CREATE_COMPLETE, because Ref UserPoolClient is a client id matching ^[\w+]+$ / ^[\w_]+$.

Actual (emulator): AssertionError: stack <name> reached 'ROLLBACK_COMPLETE', wanted 'CREATE_COMPLETE'; failures: ['<stack>: The following resource(s) failed to create: [IdentityPool]. ', 'IdentityPool: Resource handler returned message: "1 validation error detected: Value \'sim2real-xsvc-ci3-ptag-9233e448fe-UserPoolClient-85A73959E99B\' at \'cognitoIdentityProviders.1.member.clientId\' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w_]+" (RequestToken: <per-run id>, HandlerErrorCode: InvalidRequest)'].

The hyphenated Ref value is the divergence; the concrete stack name, suffix and RequestToken are per-run values, not requirements. The list index `.1.` for a one-element list is a second, smaller divergence in the same message (AWS indexes from 0).

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the id in trace_ids.
2. Minimal manual repro: cloudformation:CreateStack with a template containing (a) an AWS::Cognito::UserPool, (b) an AWS::Cognito::UserPoolClient with UserPoolId {Ref: UserPool} and GenerateSecret false, and (c) an AWS::Cognito::IdentityPool with AllowUnauthenticatedIdentities false and CognitoIdentityProviders [{ClientId: {Ref: UserPoolClient}, ProviderName: {Fn::GetAtt: [UserPool, ProviderName]}}].
3. Poll DescribeStacks: the emulator settles ROLLBACK_COMPLETE; DescribeStackEvents shows IdentityPool CREATE_FAILED with the clientId pattern violation quoted above.
4. Confirm the cause directly: DescribeStackResource(LogicalResourceId='UserPoolClient') — PhysicalResourceId is `<stack>-UserPointClient-<suffix>`-shaped (contains '-'); it must instead equal the ClientId of cognito-idp DescribeUserPoolClient / the CreateUserPoolClient response, which must match ^[\w+]+$.
5. After the fix the stack must reach CREATE_COMPLETE and the identity pool must list the provider with that client id.
