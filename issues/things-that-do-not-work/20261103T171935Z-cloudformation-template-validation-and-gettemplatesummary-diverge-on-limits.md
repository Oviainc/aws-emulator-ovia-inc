Issue-Source: trace-sweep-divergence
Issue-Trace: cfn-create-change-set-ssm-parameter-default-missing-refused
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfn-create-change-set-ssm-parameter-default-missing-refused change-set-client-token-idempotency change-set-on-stack-failure-delete-update-type-rejected cloudformation-cfns3cnt-template-summary-and-describe-route cloudformation-tsrb-template-summary-type-route template-count-limits cloudformation-cfns3cnt-update-adds-table-bucket-into-count cloudformation-eescfn-createstack-unmapped-stage-and-region cloudformation-eescfn-findinmap-and-mappings-mistakes template-body-size-limit template-summary-unrecognized-types-warning cloudformation-cfnuf-reference-diagnose-and-rollback
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateChangeSet|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|CreateChangeSet|4xx->2xx|status
Issue-Key: cloudformation|GetTemplateSummary|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|UpdateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|CreateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ValidateTemplate|2xx->4xx|$.ValidateTemplateResponse
Issue-Key: cloudformation|ValidateTemplate|2xx->4xx|status
Issue-Key: cloudformation|GetTemplateSummary|2xx->4xx|$.GetTemplateSummaryResponse
Issue-Key: cloudformation|GetTemplateSummary|2xx->4xx|status
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member
Issue-Key: cloudformation|GetTemplateSummary|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|GetTemplateSummary|4xx->2xx|status
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.Warnings
Issue-Key: cloudformation|GetTemplate|2xx->2xx|$.GetTemplateResponse.GetTemplateResult.TemplateBody
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation template validation and GetTemplateSummary diverge on limits, messages and summary fields

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplateSummary

## Description
The emulator's template-source validation, template-error wording, template limits and GetTemplateSummary projection all diverge from AWS. This bundle carries 10 distinct causes. Related open issues 20261029T214321Z-cloudformation-template-and-parameter-validation-is-not-enforced.md and 20261029T171940Z-cloudformation-template-validation-and-summary-surfaces-skip-checks.md cover neighbouring checks and own none of these replay keys (owned_by is empty for all ten), so each must be closed here.

Cause 1 of 10 - CreateChangeSet whose template declares an SSM-backed parameter whose default parameter does not exist.
AWS: 400 Sender/ValidationError, Message 'Unable to fetch parameters [<ssmParameterNames>] from parameter store for this account.' - fixed text 'Unable to fetch parameters [' / '] from parameter store for this account.', the bracketed list being the SSM parameter names the template referenced.
Emulator: 200, the change set is created.
Why: this is the refusal every CDK bootstrap-version check depends on; accepting it lets a change set be built on parameters that cannot resolve.
Fix: CreateChangeSet's parameter resolution/validation stage.
Traces: cfn-create-change-set-ssm-parameter-default-missing-refused#task#1 (cluster also covers change-set-client-token-idempotency#task#3, change-set-on-stack-failure-delete-update-type-rejected#task#3).

Cause 2 of 10 - GetTemplateSummary with more than one template source.
AWS: 400 Sender/ValidationError, Message 'Exactly one of Template URL, Template Body, Stack Name must be specified.' - entirely fixed text. Smithy: GetTemplateSummary accepts TemplateBody, TemplateURL, StackName and StackSetName as alternatives.
Emulator: 400 with 'You must specify only one of the following parameters: StackName, StackSetName, TemplateBody, or TemplateURL'.
Why: reworded refusal; message-matching callers do not recognise it.
Fix: GetTemplateSummary's template-source validation message.
Traces: cloudformation-cfns3cnt-template-summary-and-describe-route#task#4 (cluster also covers cloudformation-tsrb-template-summary-type-route#task#3, template-count-limits#task#11).

Cause 3 of 10 - UpdateStack with UsePreviousTemplate together with a template body/URL.
AWS: 400 Sender/ValidationError, Message 'You cannot specify both usePreviousTemplate and Template Body/Template URL.' - fixed text, note the lower-case 'usePreviousTemplate'. Smithy: UpdateStack takes UsePreviousTemplate, TemplateBody and TemplateURL.
Emulator: 'You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate'.
Fix: UpdateStack's template-source validation message.
Traces: cloudformation-cfns3cnt-update-adds-table-bucket-into-count#task#9.

Cause 4 of 10 - CreateStack with Fn::FindInMap naming a mapping that the template does not declare.
AWS: 400 Sender/ValidationError, Message "Template error: Mapping named '<MappingName>' is not present in the 'Mappings' section of template." - fixed text around the quoted mapping name taken from the intrinsic.
Emulator: 'Template error: Unable to get mapping for <MapName>::<TopKey>::<SecondKey>', i.e. it reports the lookup path instead of diagnosing the missing Mappings entry.
Why: the two messages point a template author at different mistakes - a missing section versus a missing key.
Fix: the Fn::FindInMap evaluator's error construction in template processing.
Traces: cloudformation-eescfn-createstack-unmapped-stage-and-region#task#3.

Cause 5 of 10 - ValidateTemplate on a template whose Fn::FindInMap keys are not resolvable at validation time.
AWS: 200 with ValidateTemplateResult.Parameters (the declared parameters with NoEcho, DefaultValue, ParameterKey) - ValidateTemplate does not evaluate mappings. Smithy: ValidateTemplateOutput declares Parameters/Description/Capabilities and no errors.
Emulator: 400.
Why: ValidateTemplate is the pre-flight call; refusing a template AWS validates blocks authoring flows outright.
Fix: ValidateTemplate's handler must not run intrinsic evaluation.
Traces: cloudformation-eescfn-findinmap-and-mappings-mistakes#task#4.

Cause 6 of 10 - GetTemplateSummary with a large TemplateBody near AWS's inline body limit.
AWS: 200 with GetTemplateSummaryResult (ResourceTypes, Metadata). Smithy: GetTemplateSummary.TemplateBody carries only length{min:1}, so the accepted inline size is the service limit, not a model constraint.
Emulator: 400.
Why: templates at the documented size are rejected, so large real templates cannot be summarised.
Fix: GetTemplateSummary's body-size validation threshold.
Traces: template-body-size-limit#task#4.

Cause 7 of 10 - GetTemplateSummary ResourceTypes for a template declaring the same type many times.
AWS: ResourceTypes contains ONE entry per declared resource, in template order (Smithy: ResourceTypes is a list of ResourceType, not a set).
Emulator: collapses repeats, so the member count does not match the template.
Why: callers size a deployment from this list; de-duplicating it understates the resource count.
Fix: GetTemplateSummary's ResourceTypes projection.
Traces: template-count-limits#task#10.

Cause 8 of 10 - GetTemplateSummary on a template that exceeds the Mappings count limit.
AWS: 400 Sender/ValidationError, Message 'Template format error: Mappings count <count> is greater than max allowed <limit>' - fixed text around the actual count from the template and the service's maximum.
Emulator: 200.
Why: a template AWS will never accept is summarised as valid, so the limit is discovered only at deploy time.
Fix: the template-limit checks in GetTemplateSummary's validation.
Traces: template-count-limits#task#15 (cluster also covers #task#16).

Cause 9 of 10 - GetTemplateSummary with TemplateSummaryConfig requesting unrecognized-type warnings.
AWS: the response carries Warnings.UnrecognizedResourceTypes. Smithy: Warnings.UnrecognizedResourceTypes is documented as 'only returned if the TemplateSummaryConfig parameter has the TreatUnrecognizedResourceTypesAsWarning configuration set to True'.
Emulator: Warnings is omitted entirely.
Why: with the warning absent a caller cannot distinguish 'no unrecognized types' from 'warnings not produced'.
Fix: GetTemplateSummary's Warnings serializer and the TemplateSummaryConfig handling.
Traces: template-summary-unrecognized-types-warning#task#4.

Cause 10 of 10 - GetTemplate does not round-trip the stored template.
AWS: GetTemplateResult.TemplateBody returns the template as it was submitted for that stack.
Emulator: returns a body that differs from the submitted template text.
Why: callers diff the returned template against their source; a non-identical round-trip reports spurious drift.
Fix: the stack's stored template body and GetTemplate's serializer must preserve the submitted text.
Traces: cloudformation-cfnuf-reference-diagnose-and-rollback#task#13.

## Evidence
Replay of simparity-wire-aws-v1, 10 divergence clusters, all stub_path='' and owned_by=[].
Cause 1: expected 400 ValidationError 'Unable to fetch parameters [/cdk-bootstrap/<qualifier>/version] from parameter store for this account.', got 200.
Cause 2: expected 'Exactly one of Template URL, Template Body, Stack Name must be specified.', got 'You must specify only one of the following parameters: StackName, StackSetName, TemplateBody, or TemplateURL'.
Cause 3: expected 'You cannot specify both usePreviousTemplate and Template Body/Template URL.', got 'You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate'.
Cause 4: expected "Template error: Mapping named 'RegionMap' is not present in the 'Mappings' section of template.", got 'Template error: Unable to get mapping for RegionMap::us-west-2::AzCount'.
Cause 5: expected 200 with ValidateTemplateResult.Parameters, got 400.
Cause 6: expected 200 with GetTemplateSummaryResult (ResourceTypes/Metadata), got 400.
Cause 7: expected ResourceTypes.member to be a list with one entry per declared resource, got a collapsed value (type_mismatch).
Cause 8: expected 400 ValidationError 'Template format error: Mappings count 201 is greater than max allowed 200', got 200.
Cause 9: expected Warnings.UnrecognizedResourceTypes present, got Warnings absent.
Cause 10: GetTemplateResult.TemplateBody value_mismatch against the submitted template.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cfn-create-change-set-ssm-parameter-default-missing-refused, step task#1 - CreateChangeSet with a template whose parameter default is an SSM parameter that does not exist.
3. Causes 2-10: replay the fixture#step bindings in trace_ids (GetTemplateSummary with two template sources, UpdateStack with UsePreviousTemplate plus a body, CreateStack/ValidateTemplate with an unresolvable Fn::FindInMap, a large template body, a repeated-type template, a template over the Mappings limit, TemplateSummaryConfig warnings, and GetTemplate round-trip).
4. Compare each response's status, Error.Message and the named result fields against the recorded AWS responses.
