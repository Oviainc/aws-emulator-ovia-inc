Issue-Source: trace-sweep-divergence
Issue-Trace: cfnerr-validate-default-outside-allowed-values
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnerr-validate-default-outside-allowed-values cfnerr-validate-unrecognized-resource-type cloudformation-template-validation-and-account-limits cfnerr-capability-auto-expand-transform cloudformation-validate-template-named-iam-and-dangling-getatt cloudformation-stack-policy-and-termination-protection cloudformation-stackset-multi-account-region-lifecycle cloudformation-template-validation-and-planning-tools cfnrep-get-template-processed-transform cloudformation-changeset-template-url-refusals cloudformation-exports-imports-limits
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ValidateTemplate|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|ValidateTemplate|4xx->2xx|status
Issue-Key: cloudformation|ValidateTemplate|2xx->2xx|$.ValidateTemplateResponse.ValidateTemplateResult.Capabilities
Issue-Key: cloudformation|ValidateTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.Version
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member[*]
Issue-Key: cloudformation|GetTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|CreateChangeSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeAccountLimits|2xx->2xx|$.DescribeAccountLimitsResponse.DescribeAccountLimitsResult.AccountLimits.member[*].Value
Issue-Key: cloudformation|DescribeAccountLimits|2xx->2xx|$.DescribeAccountLimitsResponse.DescribeAccountLimitsResult.AccountLimits.member[Name="ConcurrentResourcesLimit"].Value
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation template validation and summary surfaces skip checks and omit members

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateTemplate

## Description
The pre-deploy planning surface — ValidateTemplate, GetTemplateSummary, GetTemplate, CreateChangeSet's template fetch and DescribeAccountLimits — accepts a template AWS rejects, omits summary members, orders results differently and refuses in its own words. 10 net-new root causes, one service.

The replay reported owned_by=[] and no stub for all ten. The open issues "cloudformation: 3 net-new behavioral root causes — Parameter constraints (AllowedPattern/MinLength/MaxLength/Number range), missing-required and undeclared ..." and "CloudFormation template transforms and intrinsics are not processed ..." are the nearest neighbours; neither carries any of the fixture#step bindings below, so none of these wire-level causes is exercised today.

Cause 1 of 10: ValidateTemplate accepts a parameter whose Default is outside its AllowedValues.
AWS: Smithy: ValidateTemplate has no modelled errors, so the refusal is the service-common one — status 400, Type Sender, Code ValidationError, Message "Parameter '<parameter logical name>' must be one of AllowedValues" (fixed text apart from the offending parameter name).
Emulator: status 200 with a normal ValidateTemplateResult and no ErrorResponse.
Why: a template AWS refuses up front validates clean, so the failure surfaces only at deploy time (or never) — the whole point of the validate step is lost.
Fix: the ValidateTemplate parameter validation (Default must satisfy AllowedValues).
Traces: cfnerr-validate-default-outside-allowed-values#task#1, cfnerr-validate-unrecognized-resource-type#task#1, cloudformation-template-validation-and-account-limits#task#9.

Cause 2 of 10: ValidateTemplate omits the capability a Transform requires.
AWS: Smithy: ValidateTemplate output Capabilities (Capabilities list) and CapabilitiesReason. For a template that declares a macro Transform the recorded result carries Capabilities = [CAPABILITY_AUTO_EXPAND].
Emulator: Capabilities absent from the 200 body.
Why: a client that reads Capabilities back to decide which --capabilities to pass on CreateStack gets none, then has the create refused for a missing capability.
Fix: the ValidateTemplate handler's capability inference from the template's Transform/resource set.
Traces: cfnerr-capability-auto-expand-transform#task#2.

Cause 3 of 10: ValidateTemplate's dangling-Fn::GetAtt message is a template-format error instead of a template error.
AWS: 400 ValidationError with 'Template error: instance of Fn::GetAtt references undefined resource <the undefined logical id>' — fixed text apart from the logical id taken from the template.
Emulator: 400 with 'Template format error: Unresolved resource dependencies [<logical id>] in the Outputs block of the template'.
Why: the message is a requirement of this error class; the emulator reports a different error family ('Template format error') than the one AWS uses for an unresolved Fn::GetAtt target.
Fix: the ValidateTemplate intrinsic-resolution check's error message.
Traces: cloudformation-validate-template-named-iam-and-dangling-getatt#task#2.

Cause 4 of 10: GetTemplateSummary omits ResourceIdentifierSummaries and Version.
AWS: Smithy: GetTemplateSummary output members ResourceIdentifierSummaries (list) and Version. The recorded body carries Version = the template's AWSTemplateFormatVersion ('2010-09-09') and one ResourceIdentifierSummaries entry per resource type, each with ResourceType, LogicalResourceIds and ResourceIdentifiers (the type's primary identifier property names, e.g. TopicArn for AWS::SNS::Topic).
Emulator: both members absent.
Why: resource import cannot be planned at all — ResourceIdentifierSummaries is the member that tells a client which identifiers to supply in ResourcesToImport.
Fix: the GetTemplateSummary handler (template-version passthrough and per-resource-type identifier summary).
Traces: cloudformation-stack-policy-and-termination-protection#task#8, cloudformation-template-validation-and-account-limits#task#2, cloudformation-template-validation-and-account-limits#task#4.

Cause 5 of 10: GetTemplateSummary omits Version on the StackSet-scoped path.
AWS: as cause 4 — Version is the template's AWSTemplateFormatVersion.
Emulator: Version absent.
Why: as cause 4; this is a separate call path (summary of a StackSet's template), so it does not follow from cause 4's fix.
Fix: the GetTemplateSummary StackSet path.
Traces: cloudformation-stackset-multi-account-region-lifecycle#task#4.

Cause 6 of 10: GetTemplateSummary's ResourceTypes list is in template order rather than AWS's.
AWS: the recorded ResourceTypes list is ordered independently of the template's resource order (AWS::IAM::Role before AWS::S3::Bucket for a template that declares the bucket first), and the body also carries ResourceIdentifierSummaries for both types.
Emulator: ResourceTypes in template declaration order (AWS::S3::Bucket first) and no ResourceIdentifierSummaries.
Why: a client comparing the summary against AWS's (or a recorded baseline) sees a reordered list and a missing member.
Fix: the GetTemplateSummary serializer's ResourceTypes ordering, plus cause 4's identifier summaries.
Traces: cloudformation-template-validation-and-planning-tools#task#4.

Cause 7 of 10: GetTemplate renders the TemplateStage enum value set in the wrong order.
AWS: recorded behavior only for the ordering — the refusal is 400 ValidationError with "1 validation error detected: Value '<supplied value>' at 'templateStage' failed to satisfy constraint: Member must satisfy enum value set: [Processed, Original]". The order is the service's: Smithy TemplateStage declares [Original, Processed].
Emulator: the same sentence with '[Original, Processed]' — it renders the model's declaration order.
Why: the message is a requirement of this error class, so a client or test matching AWS's refusal text does not match.
Fix: the enum-value-set rendering in GetTemplate's input validation.
Traces: cfnrep-get-template-processed-transform#task#5.

Cause 8 of 10: CreateChangeSet's TemplateURL fetch failure is reported in the emulator's words.
AWS: when the S3 object behind TemplateURL cannot be read, the refusal is 400 ValidationError with 'S3 error: <the S3 error description>\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html' — the fixed parts are the 'S3 error: ' prefix and the trailing documentation line; the variable part is S3's own message for the underlying failure (e.g. 'The specified bucket does not exist').
Emulator: 400 with 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchBucket' — a different template that leaks the bucket/key and omits the documentation line.
Why: tooling that deploys by TemplateURL (the CLI's package/deploy flow) matches on AWS's S3-error wrapper; it does not recognise this refusal.
Fix: the TemplateURL fetch error wrapper shared by CreateChangeSet (and the other TemplateURL-taking operations).
Traces: cloudformation-changeset-template-url-refusals#task#4, #task#5, #task#6, #task#7, #task#8.

Cause 9 of 10: DescribeAccountLimits' limit set does not match the recording.
AWS: Smithy: AccountLimit {Name: LimitName (ConcurrentResourcesLimit | StackLimit | StackOutputsLimit), Value: integer}. In the recording the third AccountLimits entry carries the value 8500.
Emulator: the third entry carries 200 — i.e. the emulator's list differs from AWS's in both its ordering and its values.
Why: a client that reads a limit positionally or checks headroom before a deploy reads the wrong limit entirely.
Fix: the DescribeAccountLimits handler's limit table (names, order and values).
Traces: cloudformation-exports-imports-limits#task#4.

Cause 10 of 10: DescribeAccountLimits' ConcurrentResourcesLimit value is wrong.
AWS: the recorded AccountLimits entry named ConcurrentResourcesLimit carries Value 8500.
Emulator: 2500.
Why: as cause 9 — a pre-deploy capacity check against the concurrent-resources limit is computed from the wrong number.
Fix: the DescribeAccountLimits limit table.
Traces: cloudformation-template-validation-and-account-limits#task#5.

## Evidence
Replay of simparity-wire-aws-v1, 10 mismatch clusters:
- cfnerr-validate-default-outside-allowed-values#task#1 ValidateTemplate: status expected 400, got 200; $.ErrorResponse missing (expected Type Sender / Code ValidationError / Message "Parameter 'Flavor' must be one of AllowedValues") — 3 affected steps.
- cfnerr-capability-auto-expand-transform#task#2 ValidateTemplate 200: .Capabilities missing (expected {member: 'CAPABILITY_AUTO_EXPAND'}).
- cloudformation-validate-template-named-iam-and-dangling-getatt#task#2 ValidateTemplate 400: expected 'Template error: instance of Fn::GetAtt references undefined resource GhostFn', got 'Template format error: Unresolved resource dependencies [GhostFn] in the Outputs block of the template'.
- cloudformation-stack-policy-and-termination-protection#task#8 GetTemplateSummary 200: .ResourceIdentifierSummaries missing (expected ResourceType AWS::SNS::Topic / LogicalResourceIds [Topic] / ResourceIdentifiers [TopicArn]); .Version missing (expected '2010-09-09') — 3 affected steps.
- cloudformation-stackset-multi-account-region-lifecycle#task#4 GetTemplateSummary 200: .Version missing (expected '2010-09-09').
- cloudformation-template-validation-and-planning-tools#task#4 GetTemplateSummary 200: .ResourceTypes.member[0] expected 'AWS::IAM::Role', got 'AWS::S3::Bucket'; member[1] expected 'AWS::S3::Bucket', got 'AWS::IAM::Role'; .ResourceIdentifierSummaries missing (expected BucketName/Bucket and RoleName/Role entries).
- cfnrep-get-template-processed-transform#task#5 GetTemplate 400: expected "... enum value set: [Processed, Original]", got "... enum value set: [Original, Processed]".
- cloudformation-changeset-template-url-refusals#task#4 CreateChangeSet 400: expected 'S3 error: The specified bucket does not exist\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html', got 'S3 object does not exist. Bucket: <bucket>, Key: present.json. Error: NoSuchBucket' (5 affected steps).
- cloudformation-exports-imports-limits#task#4 DescribeAccountLimits 200: AccountLimits.member[2].Value expected '8500', got '200'.
- cloudformation-template-validation-and-account-limits#task#5 DescribeAccountLimits 200: AccountLimits.member[Name="ConcurrentResourcesLimit"].Value expected '8500', got '2500'.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. ValidateTemplate on a template whose parameter Default is not in its AllowedValues — 200 instead of 400 ValidationError; on a template with a macro Transform — Capabilities absent; on a template whose Output uses Fn::GetAtt on an undeclared resource — compare the message.
3. GetTemplateSummary on a template body, and with StackSetName set — Version and ResourceIdentifierSummaries are absent and ResourceTypes keeps template order.
4. GetTemplate(StackName=..., TemplateStage='Bogus') — compare the enum value set order in the message.
5. CreateChangeSet with a TemplateURL in a nonexistent bucket — compare the S3-error wrapper.
6. DescribeAccountLimits — compare the AccountLimits entries (order and values, in particular ConcurrentResourcesLimit) with the recording.
