Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-create-generated-template-output-member
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-create-generated-template-output-member cloudformation-generated-template-lifecycle cloudformation-generated-template-workflow cloudformation-generated-template-and-resource-scan
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateGeneratedTemplate|2xx->2xx|$.CreateGeneratedTemplateResponse.CreateGeneratedTemplateResult.Arn
Issue-Key: cloudformation|CreateGeneratedTemplate|2xx->2xx|$.CreateGeneratedTemplateResponse.CreateGeneratedTemplateResult.GeneratedTemplateId
Issue-Key: cloudformation|CreateGeneratedTemplate|2xx->2xx|$.CreateGeneratedTemplateResponse.CreateGeneratedTemplateResult.Name
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.Arn
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.Name
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.CloudFormationTemplateConfiguration
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.Resources.member.Warnings
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.TemplateConfiguration
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.Resources.member.ResourceStatusReason
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.StatusReason
Issue-Key: cloudformation|DescribeGeneratedTemplate|2xx->2xx|$.DescribeGeneratedTemplateResponse.DescribeGeneratedTemplateResult.TotalWarnings
Issue-Key: cloudformation|UpdateGeneratedTemplate|2xx->2xx|$.UpdateGeneratedTemplateResponse.UpdateGeneratedTemplateResult.Arn
Issue-Key: cloudformation|UpdateGeneratedTemplate|2xx->2xx|$.UpdateGeneratedTemplateResponse.UpdateGeneratedTemplateResult.Name
Issue-Key: cloudformation|ListGeneratedTemplates|2xx->2xx|$.ListGeneratedTemplatesResponse.ListGeneratedTemplatesResult.Summaries.member.Arn
Issue-Key: cloudformation|ListGeneratedTemplates|2xx->2xx|$.ListGeneratedTemplatesResponse.ListGeneratedTemplatesResult.Summaries.member.Name
Issue-Key: cloudformation|ListGeneratedTemplates|2xx->2xx|$.ListGeneratedTemplatesResponse.ListGeneratedTemplatesResult.Summaries.member.StatusReason
Issue-Key: cloudformation|GetGeneratedTemplate|2xx->2xx|$.GetGeneratedTemplateResponse.GetGeneratedTemplateResult.TemplateBody
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation generated-template reads omit modelled members and mis-render the template body

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGeneratedTemplate

## Description
The IaC-generator (generated template) read surface answers 200 for every operation but the response bodies are incomplete: the identity members a client uses to address the generated template are absent from Create/Describe/Update/List, the per-resource detail block is empty, and GetGeneratedTemplate returns a body that is not the generated template AWS produces. 8 net-new root causes in this one surface; all of them are in the generated-template serializers/state, none of them is a per-run value.

No open issue owns these replay keys (the replay reported owned_by=[] and no stub for every one). The nearest open issues are "cloudformation: 7 net-new root causes in the resource-scan, generated-template and type-registry surfaces..." and "cloudformation: 2 net-new root causes in the resource-scan surface..."; neither carries any of the fixture#step bindings below, so these causes are still unexercised.

Cause 1 of 8: CreateGeneratedTemplate result omits Name and Arn.
AWS: recorded behavior only — the Smithy model documents CreateGeneratedTemplateOutput with GeneratedTemplateId only, but the recorded 200 body carries CreateGeneratedTemplateResult.Name (the GeneratedTemplateName the caller supplied) and CreateGeneratedTemplateResult.Arn (arn:${Partition}:cloudformation:${Region}:${Account}:generatedtemplate/${Id}) alongside GeneratedTemplateId; the recorded GeneratedTemplateId is also shaped differently from the emulator's.
Emulator: status 200, but Name is absent, Arn is absent, and GeneratedTemplateId carries the full generatedtemplate ARN.
Why: a client that reads Name/Arn back from the create response (the CLI and CDK both echo them) sees nothing and cannot address or display the template it just created.
Fix: the CreateGeneratedTemplate handler's response serializer.
Traces: cloudformation-create-generated-template-output-member#task#1.

Cause 2 of 8: CreateGeneratedTemplate omits Name on the lifecycle path too (same member, a second recorded step that proves it is not scenario-specific).
AWS: recorded behavior only — CreateGeneratedTemplateResult.Name echoes the requested GeneratedTemplateName.
Emulator: Name missing from the 200 body.
Why: as cause 1.
Fix: the CreateGeneratedTemplate response serializer.
Traces: cloudformation-generated-template-lifecycle#task#2, cloudformation-generated-template-workflow#task#3.

Cause 3 of 8: DescribeGeneratedTemplate omits Name and Arn.
AWS: recorded behavior only — the recorded DescribeGeneratedTemplateResult carries Name (the generated template's name) and Arn (the generatedtemplate ARN) in addition to the modelled GeneratedTemplateName/GeneratedTemplateId.
Emulator: both members absent from the 200 body.
Why: a describe-then-address flow has no ARN to pass to any follow-up call.
Fix: the DescribeGeneratedTemplate response serializer.
Traces: cloudformation-generated-template-and-resource-scan#task#3.

Cause 4 of 8: DescribeGeneratedTemplate omits the template configuration and the per-resource warning block.
AWS: Smithy: DescribeGeneratedTemplate output member TemplateConfiguration (TemplateConfiguration: DeletionPolicy/UpdateReplacePolicy); the recording additionally carries CloudFormationTemplateConfiguration with the same two members and Resources.member.Warnings. For a template created with retain policies the recorded values are DeletionPolicy=RETAIN and UpdateReplacePolicy=RETAIN — i.e. the policies the create request asked for, not fixed text.
Emulator: TemplateConfiguration, CloudFormationTemplateConfiguration and Resources.member.Warnings are all absent.
Why: a caller cannot tell what deletion/update-replace policy the generated template will emit, so the retain-on-delete contract of an imported resource is invisible.
Fix: the DescribeGeneratedTemplate handler plus the generated-template state that must record the requested TemplateConfiguration.
Traces: cloudformation-generated-template-lifecycle#task#3.

Cause 5 of 8: DescribeGeneratedTemplate omits per-resource progress detail and the warning summary.
AWS: Smithy: DescribeGeneratedTemplate output members StatusReason (TemplateStatusReason), TotalWarnings (integer, range min 0) and Resources (ResourceDetails). The recording carries Resources.member.ResourceStatusReason ('Resource Template complete' for a resource whose generation completed) and Resources.member.Warnings listing one entry per unsupported property, each with Type (e.g. UNSUPPORTED_PROPERTIES), PropertyPath and Required — the paths and descriptions come from the scanned resource type's schema.
Emulator: ResourceStatusReason, Warnings, StatusReason and TotalWarnings are absent.
Why: the generator's whole value is telling the caller which properties it could not model; with no Warnings and TotalWarnings a caller believes the generated template is complete.
Fix: the DescribeGeneratedTemplate handler and the resource-detail state it reads.
Traces: cloudformation-generated-template-workflow#task#4.

Cause 6 of 8: UpdateGeneratedTemplate result omits Name and Arn.
AWS: recorded behavior only — the recorded UpdateGeneratedTemplateResult carries Name and Arn (the generatedtemplate ARN) beside GeneratedTemplateId.
Emulator: both absent from the 200 body.
Why: an update-then-fetch flow loses the template's identity.
Fix: the UpdateGeneratedTemplate response serializer.
Traces: cloudformation-generated-template-lifecycle#task#4, cloudformation-generated-template-workflow#task#7.

Cause 7 of 8: ListGeneratedTemplates summaries omit Name/Arn and carry the wrong StatusReason.
AWS: Smithy: TemplateSummary members GeneratedTemplateId/GeneratedTemplateName/Status/StatusReason/CreationTime/LastUpdatedTime/NumberOfResources; the recording additionally carries Summaries.member.Name and Summaries.member.Arn, and for a completed generation the StatusReason is 'All resources complete'.
Emulator: Name and Arn absent; StatusReason is the emulator's own 'Template complete'.
Why: listing generated templates yields entries with no addressable id, and the status reason text differs from AWS's fixed wording for the completed state.
Fix: the ListGeneratedTemplates response serializer and the status-reason text in generated-template state.
Traces: cloudformation-generated-template-workflow#task#6.

Cause 8 of 8: GetGeneratedTemplate returns a stub body, not a generated template.
AWS: Smithy: GetGeneratedTemplate output TemplateBody (length min 1), Format TemplateFormat. The recorded body is minified JSON whose Metadata block is {"AWSToolsMetrics":{"IaC_Generator":"<the generatedtemplate ARN>"}} and whose resource entry carries DeletionPolicy/UpdateReplacePolicy (from the template configuration) plus the scanned resource's properties read back from the live resource.
Emulator: a pretty-printed (newline/indented) JSON document whose Metadata block is {"TemplateId":"<arn>"}, whose single resource carries only Type and the BucketName property, and which has no DeletionPolicy/UpdateReplacePolicy.
Why: the generated template is the deliverable of this API — a client that deploys it gets a resource with none of the scanned configuration and no retain policy, and tooling keyed on the AWSToolsMetrics/IaC_Generator metadata cannot recognise the template as generator output.
Fix: the GetGeneratedTemplate template renderer (metadata key, minified serialization, policy emission and scanned-property projection).
Traces: cloudformation-generated-template-workflow#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline, 8 mismatch clusters, all status 200->200 (so the divergence is purely in the body):
- cloudformation-create-generated-template-output-member#task#1 CreateGeneratedTemplate: $.CreateGeneratedTemplateResponse.CreateGeneratedTemplateResult.Name missing (expected the requested template name, got None); .Arn missing; .GeneratedTemplateId value_mismatch (expected a shaped id, got 'arn:aws:cloudformation:us-west-2:000000000000:generatedtemplate/<uuid>').
- cloudformation-generated-template-lifecycle#task#2 CreateGeneratedTemplate: .Name missing.
- cloudformation-generated-template-and-resource-scan#task#3 DescribeGeneratedTemplate: .Name missing, .Arn missing.
- cloudformation-generated-template-lifecycle#task#3 DescribeGeneratedTemplate: .CloudFormationTemplateConfiguration missing (expected {DeletionPolicy: RETAIN, UpdateReplacePolicy: RETAIN}), .TemplateConfiguration missing (same), .Resources.member.Warnings missing, .Name missing, .Arn missing.
- cloudformation-generated-template-workflow#task#4 DescribeGeneratedTemplate: .Resources.member.ResourceStatusReason missing (expected 'Resource Template complete'), .Resources.member.Warnings missing (expected UNSUPPORTED_PROPERTIES entries with PropertyPath/Required), .CloudFormationTemplateConfiguration/.TemplateConfiguration/.StatusReason/.TotalWarnings/.Name/.Arn missing.
- cloudformation-generated-template-lifecycle#task#4 UpdateGeneratedTemplate: .Name missing, .Arn missing.
- cloudformation-generated-template-workflow#task#6 ListGeneratedTemplates: .Summaries.member.StatusReason value_mismatch (expected 'All resources complete', got 'Template complete'); .Summaries.member.Name missing; .Summaries.member.Arn missing.
- cloudformation-generated-template-workflow#task#5 GetGeneratedTemplate: .TemplateBody value_mismatch — expected minified JSON with Metadata.AWSToolsMetrics.IaC_Generator=<generatedtemplate ARN> and the resource carrying UpdateReplacePolicy/DeletionPolicy plus the scanned S3 properties; got indented JSON with Metadata.TemplateId and only Type+BucketName.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator (replay_wire_corpus).
2. Observe the 8 mismatch clusters listed under Traces, each a 200->200 body divergence on the generated-template surface.
3. Individually: CreateGeneratedTemplate(GeneratedTemplateName=<name>, Resources=[{ResourceType:AWS::S3::Bucket, ResourceIdentifier:{BucketName:<bucket>}}]) and read the result — Name/Arn are absent. Then DescribeGeneratedTemplate(<name>) — Name/Arn/TemplateConfiguration/Warnings/TotalWarnings/StatusReason are absent. Then UpdateGeneratedTemplate(<name>) — Name/Arn absent. Then ListGeneratedTemplates — Summaries entries have no Name/Arn and StatusReason is 'Template complete'. Then GetGeneratedTemplate(<name>) — the body is indented JSON with Metadata.TemplateId and no policies.
