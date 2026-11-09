Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearchserverless.cfn-collection-group-standby-disabled-read-back-then-gone cloudformation__opensearchserverless.cfn-lifecycle-policy-retention-read-back-then-gone cloudformation__opensearchserverless.cfn-lifecycle-policy-wrong-resourcetype-for-retention-rejected cloudformation__opensearchserverless.cfn-security-config-saml-read-back-then-gone
Issue-Key: cloudformation__opensearchserverless|CreateStack|ValidationError
Issue-Key: cloudformation__opensearchserverless.cfn-security-config-saml-read-back-then-gone|run#12|AssertionError|assert (config_ref, listed) == (expected_id, [expected_id]), (
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# cloudformation__opensearchserverless: 3 net-new causes — AWS::OpenSearchServerless::CollectionGroup and ::LifecyclePolicy are unregistered resource types (CreateStack refuses the template), and a stack-created ::SecurityConfig is never created in OpenSearch Serverless and Refs the bare name instead of saml/<account>/<name>

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
THREE net-new root causes, grounded in four failing behavioral checks listed under the suite heading `### cloudformation__opensearchserverless`. All three are in the emulator's CloudFormation resource-type support for OpenSearch Serverless.

=== Cause 1 of 3: AWS::OpenSearchServerless::CollectionGroup is not a registered resource type, so CreateStack refuses the whole template ===
Failing check: cloudformation__opensearchserverless.cfn-collection-group-standby-disabled-read-back-then-gone
AWS: the type is a real CloudFormation resource type backed by the Smithy-modeled opensearchserverless collection-group API (CreateCollectionGroup / BatchGetCollectionGroup / UpdateCollectionGroup / DeleteCollectionGroup are all in com.amazonaws.opensearchserverless); CreateStack accepts the template and returns a StackId, the stack reaches CREATE_COMPLETE, and the group reads back through BatchGetCollectionGroup with the declared standby-replicas setting (recorded behavior for the CFN handler itself).
Emulator: CreateStack is refused HTTP 400 `ValidationError` with the message `Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::CollectionGroup]` — the fixed text is `Template format error: Unrecognized resource types: [` + the type names from the template + `]`, so no stack is ever created and neither the round trip nor the delete can be observed.
Why: a client cannot manage OpenSearch Serverless collection groups as code; any template containing one fails at submission, which also fails every surrounding resource in the same template.
Fix: register an AWS::OpenSearchServerless::CollectionGroup resource handler in the emulator's CloudFormation resource-type registry, wired to the opensearchserverless collection-group operations for CREATE/UPDATE/DELETE and to the group's identifier for Ref.

=== Cause 2 of 3: AWS::OpenSearchServerless::LifecyclePolicy is not a registered resource type, so CreateStack refuses the whole template ===
Failing checks: cloudformation__opensearchserverless.cfn-lifecycle-policy-retention-read-back-then-gone, cloudformation__opensearchserverless.cfn-lifecycle-policy-wrong-resourcetype-for-retention-rejected
AWS: the type is a real CloudFormation resource type backed by the Smithy-modeled opensearchserverless lifecycle-policy API (CreateLifecyclePolicy / ListLifecyclePolicies / BatchGetEffectiveLifecyclePolicy / UpdateLifecyclePolicy / DeleteLifecyclePolicy); CreateStack accepts the template and returns a StackId. A retention policy declared in a stack reads back through the lifecycle-policy readers, and a policy whose rule names a ResourceType that retention does not support fails the RESOURCE (CREATE_FAILED / ROLLBACK_COMPLETE with the service's ValidationException as the resource reason), not the submission.
Emulator: CreateStack is refused HTTP 400 `ValidationError` `Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::LifecyclePolicy]` for both the valid and the invalid template, so the valid one never deploys and the invalid one is rejected for the wrong reason, at the wrong time, with the wrong error — a client cannot tell a template-level mistake from a service-level refusal.
Why: lifecycle (retention) policies cannot be deployed as code at all, and the negative class that AWS surfaces as a rolled-back stack with a service reason is surfaced instead as a submission-time template error.
Fix: register an AWS::OpenSearchServerless::LifecyclePolicy resource handler in the emulator's CloudFormation resource-type registry, wired to CreateLifecyclePolicy/UpdateLifecyclePolicy/DeleteLifecyclePolicy so that the service's own validation decides the resource's fate.

=== Cause 3 of 3: a stack-created AWS::OpenSearchServerless::SecurityConfig is never created in OpenSearch Serverless, and its Ref is the bare Name instead of the composed id saml/<account>/<name> ===
Failing check: cloudformation__opensearchserverless.cfn-security-config-saml-read-back-then-gone
AWS: the handler calls the Smithy-modeled CreateSecurityConfig(type: SecurityConfigType (required), name: ConfigName (required, 3..32, ^[a-z][a-z0-9-]+$), samlOptions: SamlConfigOptions, ...), so a CREATE_COMPLETE stack leaves a real configuration: ListSecurityConfigs(type='saml') (modeled, paginated, output securityConfigSummaries) lists a summary whose id is the composed identifier `saml/<account-id>/<name>`, GetSecurityConfig(id=<that id>) returns securityConfigDetail with that id, type 'saml' and the declared samlOptions.metadata and samlOptions.sessionTimeout, and the resource's Ref is that same id (recorded behavior: the CFN resource's primary identifier is the security config Id). After the stack is deleted GetSecurityConfig is refused ResourceNotFoundException `Security config with id: <id> does not exist` (fixed text, the id taken from the request). The account id, config name, metadata document and 60-second timeout of a run are request/state values; the `saml/<account>/<name>` id template and the listing's non-emptiness are the requirement.
Emulator: the stack reports CREATE_COMPLETE, but the Ref output is the bare declared Name (`'sim2real-secconf-dfb8488524ed'`) rather than `saml/<account>/<name>`, and ListSecurityConfigs(type='saml') contains no entry for the expected id — the configuration was never created in OpenSearch Serverless at all (`AssertionError: Ref 'sim2real-secconf-dfb8488524ed', ListSecurityConfigs ids [], expected 'saml/000000000000/sim2real-secconf-dfb8488524ed'`). The handler is stub-provisioned and synthesizes a physical id.
Why: SAML security configurations deployed through CloudFormation do not exist: no identity provider is configured for the collections that reference them, the Ref a template passes to other resources (or exports) is not an id any opensearchserverless read accepts, and GetSecurityConfig answers not-found for a resource CloudFormation reports as created.
Fix: the AWS::OpenSearchServerless::SecurityConfig resource handler in the emulator's CloudFormation resource-type registry — CREATE must call CreateSecurityConfig in the opensearchserverless state, return the service-composed `saml/<account>/<name>` id as the physical resource id (so Ref/GetAtt resolve to it), and DELETE must call DeleteSecurityConfig.

Traces (examples, by cause): Cause 1 — cloudformation__opensearchserverless.cfn-collection-group-standby-disabled-read-back-then-gone. Cause 2 — cloudformation__opensearchserverless.cfn-lifecycle-policy-retention-read-back-then-gone, cloudformation__opensearchserverless.cfn-lifecycle-policy-wrong-resourcetype-for-retention-rejected. Cause 3 — cloudformation__opensearchserverless.cfn-security-config-saml-read-back-then-gone.

## Evidence
Cause 1 — cloudformation__opensearchserverless.cfn-collection-group-standby-disabled-read-back-then-gone [error]:
  Expected (AWS): CreateStack accepts the template and returns a StackId; the stack reaches CREATE_COMPLETE and the collection group reads back with its declared standby-replicas setting.
  Actual (emulator): `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::CollectionGroup]`.

Cause 2 — cloudformation__opensearchserverless.cfn-lifecycle-policy-retention-read-back-then-gone [error] and cfn-lifecycle-policy-wrong-resourcetype-for-retention-rejected [error]:
  Expected (AWS): CreateStack accepts both templates; the valid one deploys a retention lifecycle policy that reads back, and the one whose rule names an unsupported ResourceType fails the RESOURCE and leaves the stack ROLLBACK_COMPLETE with the opensearchserverless ValidationException as the CREATE_FAILED resource reason.
  Actual (emulator), both: `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::LifecyclePolicy]` — refused at submission, so neither the deploy nor the service-level refusal happens.

Cause 3 — cloudformation__opensearchserverless.cfn-security-config-saml-read-back-then-gone [mismatch]:
  Expected (AWS): with Resources {"SecurityConfig": {"Type": "AWS::OpenSearchServerless::SecurityConfig", "Properties": {"Name": <name>, "Type": "saml", "SamlOptions": {"Metadata": <saml metadata>, "SessionTimeout": 60}}}} and Outputs {"ConfigRef": {"Ref": "SecurityConfig"}}, the stack is CREATE_COMPLETE, ConfigRef == `saml/<account-id>/<name>`, ListSecurityConfigs(type='saml') contains that id, and GetSecurityConfig(id=<it>) returns type 'saml' with the declared metadata and sessionTimeout 60; after delete_stack, GetSecurityConfig is refused ResourceNotFoundException `Security config with id: <id> does not exist`.
  Actual (emulator): stack CREATE_COMPLETE, but `AssertionError: Ref 'sim2real-secconf-dfb8488524ed', ListSecurityConfigs ids [], expected 'saml/000000000000/sim2real-secconf-dfb8488524ed'` (check line 248) — the Ref is the bare Name and the configuration exists nowhere in OpenSearch Serverless. (The name and account digits are this run's values; the `saml/<account>/<name>` template is the requirement.)

## Steps to Reproduce
Cause 1:
1. cloudformation.create_stack with a template whose Resources contain {"Type": "AWS::OpenSearchServerless::CollectionGroup", "Properties": {...standby replicas disabled...}}.
   -> EXPECTED: 200 with a StackId, then CREATE_COMPLETE and a readable collection group.
   -> ACTUAL: 400 ValidationError `Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::CollectionGroup]`.

Cause 2:
2. cloudformation.create_stack with {"Type": "AWS::OpenSearchServerless::LifecyclePolicy", "Properties": {"Name": <name>, "Type": "retention", "Policy": <retention rules>}}.
   -> EXPECTED: StackId, CREATE_COMPLETE, policy readable through the lifecycle-policy readers; deleting the stack removes it.
   -> ACTUAL: 400 ValidationError `Template format error: Unrecognized resource types: [AWS::OpenSearchServerless::LifecyclePolicy]`.
3. Repeat with a retention policy rule naming a ResourceType retention does not support.
   -> EXPECTED: the submission is accepted and the RESOURCE fails (ROLLBACK_COMPLETE, CREATE_FAILED reason carrying the opensearchserverless ValidationException).
   -> ACTUAL: the identical submission-time 400 ValidationError, so the service-level refusal is never produced.

Cause 3:
4. cloudformation.create_stack with {"Type": "AWS::OpenSearchServerless::SecurityConfig", "Properties": {"Name": <3..32 lower-case name>, "Type": "saml", "SamlOptions": {"Metadata": <saml metadata xml>, "SessionTimeout": 60}}} and Outputs {"ConfigRef": {"Ref": "SecurityConfig"}}; wait for CREATE_COMPLETE.
5. Read the stack output ConfigRef and call opensearchserverless.list_security_configs(type='saml') (paging on nextToken).
   -> EXPECTED: ConfigRef == `saml/<account-id>/<name>` and that id is listed; get_security_config(id=<it>) returns the declared samlOptions.
   -> ACTUAL: ConfigRef is the bare `<name>` and the listing contains no such id — nothing was created in OpenSearch Serverless.
6. delete_stack, then get_security_config(id=`saml/<account>/<name>`) -> expected ResourceNotFoundException `Security config with id: <id> does not exist`; unreachable because the resource never existed.
