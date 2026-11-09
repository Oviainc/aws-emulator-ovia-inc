Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr.deploy-ecr-registry-singletons-provisioned cloudformation__ecr.deploy-ecr-repository-creation-template-provisioned cloudformation__ecr.cfn-ecr-repository-name-string-constraints-rejected cloudformation__ecr.deploy-ecr-repository-duplicate-name-rejected cloudformation__ecr.deploy-ecr-pull-through-cache-rule-provisioned cloudformation__ecr.deploy-ecr-pull-through-cache-rule-duplicate-prefix-rejected cloudformation__ecr.deploy-ecr-repository-name-change-replaces-mutability-change-updates-in-place cloudformation__ecr.deploy-ecr-repository-rename-collision-blocks-replacement-rolls-back
Issue-Key: cloudformation__ecr|CreateStack|ValidationError
Issue-Key: cloudformation__ecr.cfn-ecr-repository-name-string-constraints-rejected|run#18|AssertionError|assert _masked(reason) == _PATTERN_REASON, f"pattern violation reason: {reason!r}"
Issue-Key: cloudformation__ecr.deploy-ecr-repository-duplicate-name-rejected|run#22|AssertionError|assert stack_failed_reasons == [_STACK_REASON], (
Issue-Key: cloudformation__ecr.deploy-ecr-pull-through-cache-rule-provisioned|run#10|AssertionError|assert outputs["Prefix"] == prefix, (
Issue-Key: ecr|DescribePullThroughCacheRules|PullThroughCacheRuleNotFoundException
Issue-Key: cloudformation__ecr.deploy-ecr-repository-name-change-replaces-mutability-change-updates-in-place|run#32|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__ecr.deploy-ecr-repository-rename-collision-blocks-replacement-rolls-back|run#19|AssertionError|assert _in_order(rows, [
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__ecr: 8 net-new causes — two AWS::ECR::* resource types unrecognized, CFN handler refusal/validation reasons are ECR-API worded, Ref of a pull-through cache rule is wrong, a stack-created rule is invisible to ECR, and in-place update / failed-replacement rollback diverge

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight distinct root causes in CloudFormation's handling of the AWS::ECR::* resource types. All eight are in the cloudformation__ecr service and must each be closed.

Cause 1 of 8: AWS::ECR::RegistryScanningConfiguration is not a registered resource type.
AWS: CreateStack accepts a template declaring AWS::ECR::RegistryScanningConfiguration (the registry-singleton resource) and provisions it, so the registry scanning configuration is readable afterwards through ecr:GetRegistryScanningConfiguration. Recorded behavior only for the stack transcript; the type is a published CloudFormation resource type.
Emulator: CreateStack is refused 400 ValidationError with message 'Template format error: Unrecognized resource types: [AWS::ECR::RegistryScanningConfiguration]'. Fixed text 'Template format error: Unrecognized resource types: [' + the declared type names + ']'.
Why: a client cannot deploy the registry singletons by template at all; the stack never exists, so every downstream read of registry configuration is unreachable.
Fix: the CloudFormation resource-type registry / ECR resource-type handlers.
Traces: cloudformation__ecr.deploy-ecr-registry-singletons-provisioned.

Cause 2 of 8: AWS::ECR::RepositoryCreationTemplate is not a registered resource type.
AWS: CreateStack provisions AWS::ECR::RepositoryCreationTemplate and the template is then readable through ecr:DescribeRepositoryCreationTemplates. Recorded behavior only.
Emulator: CreateStack is refused 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ECR::RepositoryCreationTemplate]'.
Why: repository creation templates cannot be declared in infrastructure-as-code.
Fix: the CloudFormation resource-type registry / ECR resource-type handlers.
Traces: cloudformation__ecr.deploy-ecr-repository-creation-template-provisioned.

Cause 3 of 8: a repositoryName that violates the resource schema is reported with ECR's API parameter message, not CloudFormation's model-validation reason.
AWS: when AWS::ECR::Repository declares a RepositoryName that breaks the schema's pattern/maxLength/minLength, the stack rolls back and the resource's CREATE_FAILED reason is the handler-returned model-validation sentence: 'Resource handler returned message: "Model validation failed (#/RepositoryName: failed validation constraint for keyword [pattern])" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — fixed text 'Resource handler returned message: "Model validation failed (#/' + the property path + ': failed validation constraint for keyword [' + the violated keyword, i.e. pattern | maxLength | minLength + '])" (RequestToken: ' + the per-request token + ', HandlerErrorCode: InvalidRequest)'. Smithy (ECR): RepositoryName @length min 2 max 256, @pattern ^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$ — those are the same bounds the resource schema states.
Emulator: the reason is the ECR API's own parameter refusal relayed through the handler: 'Resource handler returned message: "Invalid parameter at \'repositoryName\' failed to satisfy constraint: \'must satisfy regular expression \'[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*)*\'\'" (RequestToken: …, HandlerErrorCode: InvalidRequest)'. The HandlerErrorCode and the rollback are right; only the message class is wrong (the maxLength and minLength legs of the same check assert the identical [maxLength]/[minLength] wording).
Why: a client that keys off the model-validation sentence (or greps for the offending property path '#/RepositoryName') cannot tell which schema keyword it broke.
Fix: the AWS::ECR::Repository resource handler's property validation (validate against the resource schema before calling the ECR API, and author the reason in CloudFormation's wording).
Traces: cloudformation__ecr.cfn-ecr-repository-name-string-constraints-rejected.

Cause 4 of 8: a create that fails on a duplicate repository name records no stack-level CREATE_FAILED reason.
AWS: besides the resource-level CREATE_FAILED event, the stack itself gets a CREATE_FAILED event whose reason is the fixed sentence 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.' (the count is the number of failed resources).
Emulator: DescribeStackEvents yields no stack-level CREATE_FAILED reason at all (the collected list is empty).
Why: a client that reads the stack-level failure reason to decide whether to call DescribeEvents sees an empty reason and reports no cause for the rollback.
Fix: the CloudFormation stack event recorder for a failed create (stack-level event emission).
Traces: cloudformation__ecr.deploy-ecr-repository-duplicate-name-rejected.

Cause 5 of 8: Ref of AWS::ECR::PullThroughCacheRule does not resolve to the EcrRepositoryPrefix.
AWS: the primary identifier of AWS::ECR::PullThroughCacheRule is its EcrRepositoryPrefix, so Ref returns that prefix.
Emulator: Ref returns a generated physical id of the form '<stack-name>-<logical-id>-<suffix>' instead of the declared prefix.
Why: any template that passes Ref of the rule to another resource (or an Output a client reads to address the cached repository) receives a string that is not a repository prefix, so the derived repository name never exists.
Fix: the AWS::ECR::PullThroughCacheRule handler's physical-id/Ref resolution.
Traces: cloudformation__ecr.deploy-ecr-pull-through-cache-rule-provisioned.

Cause 6 of 8: a pull-through cache rule provisioned by a stack is not registered in ECR.
AWS: once the stack reports the rule created, ecr:DescribePullThroughCacheRules lists it.
Emulator: DescribePullThroughCacheRules answers 400 PullThroughCacheRuleNotFoundException 'A pull-through cache rule with repository prefix \'<prefix>\' does not exist in the registry with id \'<registryId>\'' (fixed text with the requested prefix and the registry id from state), so the duplicate-prefix scenario behind it can never be set up.
Why: the stack claims a resource that the service does not have; a client cannot read, update or detect a conflict on a rule it just deployed.
Fix: the AWS::ECR::PullThroughCacheRule handler's create path (it must write through to ECR registry state).
Traces: cloudformation__ecr.deploy-ecr-pull-through-cache-rule-duplicate-prefix-rejected.

Cause 7 of 8: an update that changes only a mutable property rolls the stack back instead of updating the repository in place.
AWS: changing AWS::ECR::Repository's ImageTagMutability is an in-place update — UpdateStack reaches UPDATE_COMPLETE and the existing repository keeps its physical id with the new mutability; only a RepositoryName change forces a replacement.
Emulator: the update ends UPDATE_ROLLBACK_COMPLETE with StackStatusReason 'The following resource(s) failed to update: [<logical id>]. ', so the mutability change is lost.
Why: a client cannot change a repository's tag mutability by template at all; every such deploy appears to fail.
Fix: the AWS::ECR::Repository handler's update path (in-place vs replacement classification).
Traces: cloudformation__ecr.deploy-ecr-repository-name-change-replaces-mutability-change-updates-in-place.

Cause 8 of 8: a replacement blocked by a name collision does not roll back to the original physical id.
AWS: when a RepositoryName change forces a replacement and the new name is taken, the resource reports the replacement attempt, fails it with the AlreadyExists reason, and then rolls back to the original repository id — three events in that order, the stack ending UPDATE_ROLLBACK_COMPLETE with the original resource intact.
Emulator: only two events are recorded — CREATE_IN_PROGRESS with an empty reason, then CREATE_FAILED with 'Resource handler returned message: "The repository with name \'<name>\' already exists in the registry with id \'<registryId>\'" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)'. The rollback event that restores the original physical id is never emitted (the third entry is absent).
Why: a client reading the resource's event history cannot see that the original repository survived, and automation that waits for the rollback entry before retrying never observes it.
Fix: the CloudFormation update-rollback event recorder for a failed resource replacement.
Traces: cloudformation__ecr.deploy-ecr-repository-rename-collision-blocks-replacement-rolls-back.

## Evidence
Replay of the behavioral suite (cloudformation__ecr 1/9 passed):

1) deploy-ecr-registry-singletons-provisioned — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ECR::RegistryScanningConfiguration]
2) deploy-ecr-repository-creation-template-provisioned — ... Unrecognized resource types: [AWS::ECR::RepositoryCreationTemplate]
3) cfn-ecr-repository-name-string-constraints-rejected — expected masked reason 'Resource handler returned message: "Model validation failed (#/RepositoryName: failed validation constraint for keyword [pattern])" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'; got 'Resource handler returned message: "Invalid parameter at \'repositoryName\' failed to satisfy constraint: \'must satisfy regular expression \'[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*)*\'\'" (RequestToken: …, HandlerErrorCode: InvalidRequest)'
4) deploy-ecr-repository-duplicate-name-rejected — AssertionError: expected the stack's CREATE_FAILED reason to be 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.', got []
5) deploy-ecr-pull-through-cache-rule-provisioned — AssertionError: Ref of PullThroughCacheRule must be the EcrRepositoryPrefix '<prefix>', got '<stack>-Rule-<suffix>'
6) deploy-ecr-pull-through-cache-rule-duplicate-prefix-rejected — botocore.errorfactory.PullThroughCacheRuleNotFoundException: ... when calling the DescribePullThroughCacheRules operation: A pull-through cache rule with repository prefix '<prefix>' does not exist in the registry with id '000000000000'
7) deploy-ecr-repository-name-change-replaces-mutability-change-updates-in-place — update ended StackStatus 'UPDATE_ROLLBACK_COMPLETE', StackStatusReason 'The following resource(s) failed to update: [Modified]. '
8) deploy-ecr-repository-rename-collision-blocks-replacement-rolls-back — observed events [('CREATE_IN_PROGRESS','',''), ('CREATE_FAILED','Resource handler returned message: "The repository with name … already exists in the registry with id …" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)', None)]; the rollback-to-original entry is missing

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the behavioral checks listed in trace_ids (service cloudformation__ecr).
3. Per cause: (1,2) CreateStack with a template declaring AWS::ECR::RegistryScanningConfiguration / AWS::ECR::RepositoryCreationTemplate; (3) CreateStack with AWS::ECR::Repository RepositoryName breaking the pattern, then read the resource's CREATE_FAILED reason; (4) CreateStack declaring a repository whose name already exists, then DescribeStackEvents for the stack-level CREATE_FAILED reason; (5,6) CreateStack with AWS::ECR::PullThroughCacheRule, then read Ref/Outputs and call DescribePullThroughCacheRules; (7) UpdateStack changing only ImageTagMutability; (8) UpdateStack changing RepositoryName to a name already taken and read the resource event history.
