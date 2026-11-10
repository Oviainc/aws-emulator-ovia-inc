Issue-Source: trace-sweep-divergence
Issue-Trace: cfn-describe-stacks-account-listing-membership
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfn-describe-stacks-account-listing-membership cloudformation-generated-template-workflow cloudformation-template-validation-and-account-limits template-summary-unrecognized-types-warning
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member[*]
Issue-Key: cloudformation|ListGeneratedTemplates|2xx->2xx|$.ListGeneratedTemplatesResponse.ListGeneratedTemplatesResult.Summaries.member[*]
Issue-Key: cloudformation|EstimateTemplateCost|2xx->2xx|$.EstimateTemplateCostResponse.EstimateTemplateCostResult.Url
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.Warnings
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudformation response payloads diverge: DescribeStacks rollback flags, generated-template summary members, cost-estimate URL host and empty Warnings struct

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Four distinct root causes, all in how CloudFormation read operations serialize their successful responses. Each is numbered with its own fixture and step; all four must be closed before this issue is done.

Cause 1 of 4: the account-wide DescribeStacks listing reports rollback state the recorded service does not.
AWS: DescribeStacks with no StackName returns every stack in the account; each Stack carries DisableRollback and DeploymentConfig.DisableRollback describing whether rollback is disabled for the stack (Smithy: Stack.DisableRollback -> boolean, Stack.DeploymentConfig -> DeploymentConfig). In the recording both probe stacks come back with DisableRollback 'false' and DeploymentConfig.DisableRollback 'false'.
Emulator: the entry for the stack recorded with DisableRollback 'false' comes back with DisableRollback 'true' and DeploymentConfig.DisableRollback 'true', so neither recorded Stacks member matches the served listing (member[0] and member[1] both report missing). Hypothesis: the emulator persists the create-time rollback/OnFailure request flag onto the stack record and echoes it back, where AWS reports the flag as it stands for the completed stack.
Why: a client reading DescribeStacks to decide whether a stack can be updated in place, or whether a failed create will be rolled back, reads the opposite of AWS for every stack created with that flag — and a listing membership check over the account sees neither stack as it recorded them.
Fix: the DescribeStacks serializer / the stack state that backs Stack.DisableRollback and Stack.DeploymentConfig.
Traces: cfn-describe-stacks-account-listing-membership#task#5.

Cause 2 of 4: ListGeneratedTemplates summaries omit members AWS returns and use a different StatusReason.
AWS: recorded behavior only for the extra members — the recorded ListGeneratedTemplatesResult.Summaries member carries Arn and Name alongside GeneratedTemplateId, GeneratedTemplateName, Status, StatusReason, CreationTime, LastUpdatedTime and NumberOfResources (Smithy TemplateSummary models only the latter set, so Arn/Name are wire-recorded additions), and the StatusReason for a completed generation is the fixed text 'All resources complete'.
Emulator: the summary has no Arn and no Name member, and StatusReason is 'Template complete'. (The emulator also serializes the timestamps with microsecond precision, e.g. '...T09:48:27.53036Z', where the recording shows millisecond precision, e.g. '...T10:16:34.504Z'; the instant itself is a per-run value, the precision is not.)
Why: a client that reads the generated template's Arn/Name straight out of the listing gets nothing, and status reporting that surfaces StatusReason shows text AWS never emits.
Fix: the ListGeneratedTemplates serializer (the TemplateSummary projection) and the generated-template status-reason text.
Traces: cloudformation-generated-template-workflow#task#6.

Cause 3 of 4: EstimateTemplateCost returns a cost-calculator URL on a host the emulator does not serve.
AWS: EstimateTemplateCost returns Url, an AWS-hosted estimate link for the submitted template (Smithy: EstimateTemplateCostOutput.Url).
Emulator: returns 'https://calculator.aws/#/estimate?id=cloudformation/<hash>' — the real public calculator host, which this emulator does not serve, so the replay flags it host_not_own.
Why: the emulator announces an address other than the one it is reached on, so a client that follows the returned Url leaves the emulated environment (or fails closed in an offline one) instead of reaching the emulator.
Fix: the EstimateTemplateCost handler's URL construction — it must publish a host the emulator serves.
Traces: cloudformation-template-validation-and-account-limits#task#6.

Cause 4 of 4: GetTemplateSummary serializes Warnings as an empty element instead of a Warnings structure.
AWS: when TemplateSummaryConfig.TreatUnrecognizedResourceTypesAsWarning is set, GetTemplateSummaryResult.Warnings is a Warnings structure containing the UnrecognizedResourceTypes element (Smithy: Warnings structure, member UnrecognizedResourceTypes -> ResourceTypes list, documented as 'A list of all of the unrecognized resource types'); the recording shows <Warnings> carrying an UnrecognizedResourceTypes child.
Emulator: emits Warnings with no UnrecognizedResourceTypes child at all, so the member deserializes as an empty scalar rather than a structure (type_mismatch).
Why: an SDK that expects Warnings.UnrecognizedResourceTypes gets a scalar where a structure/list is modelled — the unrecognized-type warning is silently lost, and strict deserializers error.
Fix: the GetTemplateSummary serializer's Warnings member (emit the structure with its UnrecognizedResourceTypes list, including when the list is empty).
Traces: template-summary-unrecognized-types-warning#task#4.

## Evidence
Cause 1 (DescribeStacks, status 200 both sides): $.DescribeStacksResponse.DescribeStacksResult.Stacks.member[0] and .member[1]: missing — recorded members carry 'DisableRollback': 'false' and 'DeploymentConfig': {'Mode': 'STANDARD', 'DisableRollback': 'false'}; the served entry for the same stack carries 'DisableRollback': 'true' and 'DeploymentConfig': {'Mode': 'STANDARD', 'DisableRollback': 'true'}.
Cause 2 (ListGeneratedTemplates, status 200 both sides): $.ListGeneratedTemplatesResponse.ListGeneratedTemplatesResult.Summaries.member[0]: missing — recorded member has 'Arn', 'Name' and 'StatusReason': 'All resources complete'; served member has no Arn, no Name and 'StatusReason': 'Template complete'.
Cause 3 (EstimateTemplateCost, status 200 both sides): $.EstimateTemplateCostResponse.EstimateTemplateCostResult.Url: host_not_own (got 'https://calculator.aws/#/estimate?id=cloudformation/f6fadf4af00e2eb3f2043f1962b8c04a677b58b5').
Cause 4 (GetTemplateSummary, status 200 both sides): $.GetTemplateSummaryResponse.GetTemplateSummaryResult.Warnings: type_mismatch (expected {'UnrecognizedResourceTypes': ''}, got '').

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture cfn-describe-stacks-account-listing-membership step task#5 — create the two probe stacks, call DescribeStacks with no StackName and compare each Stack's DisableRollback and DeploymentConfig.
3. Cause 2: replay fixture cloudformation-generated-template-workflow step task#6 — CreateGeneratedTemplate, wait for COMPLETE, then ListGeneratedTemplates and inspect Summaries[0] for Arn, Name and StatusReason.
4. Cause 3: replay fixture cloudformation-template-validation-and-account-limits step task#6 — call EstimateTemplateCost and inspect the returned Url's host.
5. Cause 4: replay fixture template-summary-unrecognized-types-warning step task#4 — call GetTemplateSummary with TemplateSummaryConfig.TreatUnrecognizedResourceTypesAsWarning=true on a template with an unknown type and inspect the Warnings member.
