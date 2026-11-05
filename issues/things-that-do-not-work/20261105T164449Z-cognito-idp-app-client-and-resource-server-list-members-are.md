Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-auth-challenge-flow
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-auth-challenge-flow cognito-idp-user-settings-and-mfa cip-autherr-enabled-ghost-not-authorized cip-autherr-forgot-password-ghost-enabled-fabricated cip-autherr-legacy-ghost-user-not-found cognito-idp-app-client-crud-and-secret-lifecycle cognito-idp-self-service-signup-and-token-session cognito-idp-user-pool-client-domain-and-replica-configuration cognito-idp-resource-server-and-oauth-scope-enforcement
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.AllowedOAuthScopes[*]
Issue-Key: cognito-idp|UpdateUserPoolClient|2xx->2xx|$.UserPoolClient.ExplicitAuthFlows[*]
Issue-Key: cognito-idp|UpdateResourceServer|2xx->2xx|$.ResourceServer.Scopes[*].ScopeDescription
Issue-Key: cognito-idp|UpdateResourceServer|2xx->2xx|$.ResourceServer.Scopes[*].ScopeName
Issue-Key: cognito-idp|ListResourceServers|2xx->2xx|$.ResourceServers[Name="probe-api"].Scopes[*].ScopeDescription
Issue-Key: cognito-idp|ListResourceServers|2xx->2xx|$.ResourceServers[Name="probe-api"].Scopes[*].ScopeName
Issue-Commit: 580c4da031f2890ff11393cc3a031fb2021c1c07

# App-client and resource-server list members are re-ordered instead of preserving the order the request supplied

> _Note: this issue was identified against the code base at commit `580c4da031f2890ff11393cc3a031fb2021c1c07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
This bundle carries 8 causes, all of the same fault: every list-valued member of an app client (ExplicitAuthFlows, AllowedOAuthScopes) and of a resource server (Scopes) is serialized in an emulator-internal order instead of the order the caller supplied, on both the write response and every later read. All 8 causes must be closed before submitting.

Shared root cause. AWS: ExplicitAuthFlowsListType, ScopeListType and ResourceServerScopeListType are ordinary Smithy lists — Smithy: CreateUserPoolClient.ExplicitAuthFlows -> ExplicitAuthFlowsListType (type list, no @uniqueItems, no sort trait); CreateUserPoolClient.AllowedOAuthScopes -> ScopeListType (@length min 0 max 50, type list); UpdateResourceServer.Scopes -> ResourceServerScopeListType (@length min 0 max 100, type list). A list has wire order, and AWS echoes the member order of the request both in the mutating operation's response and in every subsequent Describe/List read; it neither sorts the members nor canonicalizes them to the enum declaration order. The recordings confirm this: different fixtures send the same three ALLOW_* flows in different orders and AWS echoes each fixture's own order, so no fixed order can be correct. Emulator: it emits one fixed order regardless of the request, so for each fixture the members come back permuted. Why: a client that round-trips a client/resource-server definition (read-modify-write, or an IaC tool such as CDK/Terraform/CloudFormation that diffs the live list against its desired list element-by-element) sees a permanent spurious difference and either reports endless drift or rewrites the resource on every apply; code that indexes a list positionally (ExplicitAuthFlows[0], Scopes[1].ScopeName) reads the wrong member. Fix: the state layer must store these lists as ordered sequences exactly as received and the serializers for CreateUserPoolClient / UpdateUserPoolClient / DescribeUserPoolClient / UpdateResourceServer / ListResourceServers must emit them in that stored order rather than re-deriving an order.

Cause 1 of 8: CreateUserPoolClient re-orders ExplicitAuthFlows in its own response. AWS: echoes the requested order (recorded expected ExplicitAuthFlows[0]='ALLOW_USER_PASSWORD_AUTH', [1]='ALLOW_ADMIN_USER_PASSWORD_AUTH'). Emulator: returns [0]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [1]='ALLOW_USER_PASSWORD_AUTH', status 200 in both. Why/Fix: as above (CreateUserPoolClient serializer + state). Traces: cognito-idp-admin-auth-challenge-flow#setup#2 (also cognito-idp-user-settings-and-mfa#setup#3).

Cause 2 of 8: CreateUserPoolClient re-orders a three-element ExplicitAuthFlows list. AWS: expected [0]='ALLOW_USER_PASSWORD_AUTH', [1]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [2]='ALLOW_REFRESH_TOKEN_AUTH'. Emulator: [0]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [1]='ALLOW_REFRESH_TOKEN_AUTH', [2]='ALLOW_USER_PASSWORD_AUTH', status 200. Note this request's order differs from Cause 1's, which is why a fixed emulator order cannot satisfy both. Traces: cognito-idp-self-service-signup-and-token-session#task#2.

Cause 3 of 8: DescribeUserPoolClient re-orders ExplicitAuthFlows on read. AWS: expected [0]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [1]='ALLOW_REFRESH_TOKEN_AUTH', [2]='ALLOW_USER_PASSWORD_AUTH'. Emulator: [0]='ALLOW_USER_PASSWORD_AUTH', [1]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [2]='ALLOW_REFRESH_TOKEN_AUTH', status 200. Traces: cip-autherr-enabled-ghost-not-authorized#task#6 (also cip-autherr-forgot-password-ghost-enabled-fabricated#task#3, cip-autherr-legacy-ghost-user-not-found#task#6).

Cause 4 of 8: DescribeUserPoolClient re-orders both AllowedOAuthScopes and ExplicitAuthFlows. AWS: expected AllowedOAuthScopes[0]='email', [1]='openid' and ExplicitAuthFlows[0]='ALLOW_REFRESH_TOKEN_AUTH', [1]='ALLOW_USER_PASSWORD_AUTH'. Emulator: AllowedOAuthScopes[0]='openid', [1]='email' and ExplicitAuthFlows[0]='ALLOW_USER_PASSWORD_AUTH', [1]='ALLOW_REFRESH_TOKEN_AUTH', status 200. This cause shows the fault is not confined to the auth-flow enum list. Traces: cognito-idp-app-client-crud-and-secret-lifecycle#task#3.

Cause 5 of 8: DescribeUserPoolClient re-orders a two-element ExplicitAuthFlows list. AWS: expected [0]='ALLOW_REFRESH_TOKEN_AUTH', [1]='ALLOW_USER_PASSWORD_AUTH'. Emulator: [0]='ALLOW_USER_PASSWORD_AUTH', [1]='ALLOW_REFRESH_TOKEN_AUTH', status 200. Traces: cognito-idp-user-pool-client-domain-and-replica-configuration#task#3.

Cause 6 of 8: UpdateUserPoolClient re-orders ExplicitAuthFlows in its own response, so the update's effect cannot be confirmed from its result. AWS: expected [1]='ALLOW_ADMIN_USER_PASSWORD_AUTH', [2]='ALLOW_REFRESH_TOKEN_AUTH'. Emulator: [1]='ALLOW_REFRESH_TOKEN_AUTH', [2]='ALLOW_ADMIN_USER_PASSWORD_AUTH', status 200. Fix: UpdateUserPoolClient serializer + state. Traces: cognito-idp-user-pool-client-domain-and-replica-configuration#task#4.

Cause 7 of 8: UpdateResourceServer does not return the scope list in the order the update supplied. AWS: expected Scopes[1].ScopeName='admin' / ScopeDescription='Admin access' and Scopes[2].ScopeName='write' / ScopeDescription='Write access'. Emulator: Scopes[1] is write/'Write access' and Scopes[2] is admin/'Admin access', status 200 — i.e. the request's scope ordering is not adopted by the stored resource server. Fix: UpdateResourceServer handler/state must replace the scope sequence with the request's sequence. Traces: cognito-idp-resource-server-and-oauth-scope-enforcement#task#4.

Cause 8 of 8: ListResourceServers reports the same permuted scope list, confirming the wrong order is persisted and not merely a response-time artifact. AWS: expected ResourceServers[Name="probe-api"].Scopes[1] = admin/'Admin access', Scopes[2] = write/'Write access'. Emulator: Scopes[1] = write/'Write access', Scopes[2] = admin/'Admin access', status 200. Fix: ListResourceServers serializer + resource-server state. Traces: cognito-idp-resource-server-and-oauth-scope-enforcement#task#5.

Related already-open issue: 20261031T161752Z-cognito-idp-getcsvheader-returns-the-pool-s-columns-in-the.md reports the same class of ordering fault for GetCSVHeader only; it carries no replay key for any app-client or resource-server step, so it does not cover these 8 causes.

## Evidence
Replayed against the emulator booted from this checkout (trace set simparity-wire-aws-v1, 193 fixtures). All eight clusters are verdict=mismatch with expected_status == actual_status == 200; only member order differs.

Cause 1 — CreateUserPoolClient, cognito-idp-admin-auth-challenge-flow#setup#2:
  $.UserPoolClient.ExplicitAuthFlows[0]: expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_ADMIN_USER_PASSWORD_AUTH'
  $.UserPoolClient.ExplicitAuthFlows[1]: expected 'ALLOW_ADMIN_USER_PASSWORD_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'
Cause 2 — CreateUserPoolClient, cognito-idp-self-service-signup-and-token-session#task#2:
  [0] expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_ADMIN_USER_PASSWORD_AUTH'; [1] expected 'ALLOW_ADMIN_USER_PASSWORD_AUTH', got 'ALLOW_REFRESH_TOKEN_AUTH'; [2] expected 'ALLOW_REFRESH_TOKEN_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'
Cause 3 — DescribeUserPoolClient, cip-autherr-enabled-ghost-not-authorized#task#6:
  [0] expected 'ALLOW_ADMIN_USER_PASSWORD_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'; [1] expected 'ALLOW_REFRESH_TOKEN_AUTH', got 'ALLOW_ADMIN_USER_PASSWORD_AUTH'; [2] expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_REFRESH_TOKEN_AUTH'
Cause 4 — DescribeUserPoolClient, cognito-idp-app-client-crud-and-secret-lifecycle#task#3:
  $.UserPoolClient.AllowedOAuthScopes[0]: expected 'email', got 'openid'; [1] expected 'openid', got 'email'
  $.UserPoolClient.ExplicitAuthFlows[0]: expected 'ALLOW_REFRESH_TOKEN_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'; [1] expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_REFRESH_TOKEN_AUTH'
Cause 5 — DescribeUserPoolClient, cognito-idp-user-pool-client-domain-and-replica-configuration#task#3:
  [0] expected 'ALLOW_REFRESH_TOKEN_AUTH', got 'ALLOW_USER_PASSWORD_AUTH'; [1] expected 'ALLOW_USER_PASSWORD_AUTH', got 'ALLOW_REFRESH_TOKEN_AUTH'
Cause 6 — UpdateUserPoolClient, cognito-idp-user-pool-client-domain-and-replica-configuration#task#4:
  [1] expected 'ALLOW_ADMIN_USER_PASSWORD_AUTH', got 'ALLOW_REFRESH_TOKEN_AUTH'; [2] expected 'ALLOW_REFRESH_TOKEN_AUTH', got 'ALLOW_ADMIN_USER_PASSWORD_AUTH'
Cause 7 — UpdateResourceServer, cognito-idp-resource-server-and-oauth-scope-enforcement#task#4:
  $.ResourceServer.Scopes[1].ScopeName: expected 'admin', got 'write'; .ScopeDescription: expected 'Admin access', got 'Write access'
  $.ResourceServer.Scopes[2].ScopeName: expected 'write', got 'admin'; .ScopeDescription: expected 'Write access', got 'Admin access'
Cause 8 — ListResourceServers, cognito-idp-resource-server-and-oauth-scope-enforcement#task#5:
  $.ResourceServers[Name="probe-api"].Scopes[1].ScopeName: expected 'admin', got 'write'; Scopes[2].ScopeName: expected 'write', got 'admin' (descriptions swapped likewise)

Cross-fixture proof that AWS is not sorting: Cause 1 expects ALLOW_USER_PASSWORD_AUTH before ALLOW_ADMIN_USER_PASSWORD_AUTH while Cause 3 expects ALLOW_ADMIN_USER_PASSWORD_AUTH first — each matches its own request, so the only correct behavior is to preserve the submitted order.

## Steps to Reproduce
1. Replay the wire corpus against the emulator built from this checkout (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. App-client order: replay fixture cognito-idp-admin-auth-challenge-flow step setup#2 (CreateUserPoolClient) and compare $.UserPoolClient.ExplicitAuthFlows element-by-element with the request's ExplicitAuthFlows; repeat with cognito-idp-self-service-signup-and-token-session#task#2, cip-autherr-enabled-ghost-not-authorized#task#6, cognito-idp-app-client-crud-and-secret-lifecycle#task#3 (also check AllowedOAuthScopes), cognito-idp-user-pool-client-domain-and-replica-configuration#task#3 and #task#4.
3. Resource-server order: replay cognito-idp-resource-server-and-oauth-scope-enforcement#task#4 (UpdateResourceServer) and then #task#5 (ListResourceServers) and compare $.ResourceServer.Scopes / $.ResourceServers[Name="probe-api"].Scopes positionally with the Scopes list sent by the update.
4. In every case the status is 200 and the members are the same set, but at least two positions are transposed relative to the request.
