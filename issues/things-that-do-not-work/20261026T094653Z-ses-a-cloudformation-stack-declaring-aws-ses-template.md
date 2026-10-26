Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.deploy-ses-template-cfn-renders
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|GetTemplate|TemplateDoesNotExist
Issue-Commit: bb568fea88423ace8b4c8a03ddba0a6ee612948a

# ses: a CloudFormation stack declaring AWS::SES::Template reaches CREATE_COMPLETE without creating the template — GetTemplate then answers TemplateDoesNotExist

> _Note: this issue was identified against the code base at commit `bb568fea88423ace8b4c8a03ddba0a6ee612948a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplate

## Description
One net-new behavioral root cause on the SES surface. The failing check below was NOT marked ALREADY FILED and is NOT OWNED by any open issue in the replay report, so no open issue carries its replay key.

Cause 1 of 1: a CloudFormation stack whose template declares `AWS::SES::Template` reaches CREATE_COMPLETE without provisioning the SES email template — the stack record exists, the template does not, and the first SES read after the deploy is refused.

AWS: recorded behavior only (CloudFormation resource types are not modeled in the SES Smithy model; the SES-side reads are). A stack declaring `AWS::SES::Template` with Properties.Template = {TemplateName: <n>, SubjectPart, HtmlPart, TextPart} reaches CREATE_COMPLETE and the resource is then a fully functional SES template, equivalent to one created by CreateTemplate (Smithy: com.amazonaws.ses#CreateTemplate, input {Template required}, errors AlreadyExists/InvalidTemplate/LimitExceeded): GetTemplate(TemplateName=<n>) (Smithy: com.amazonaws.ses#GetTemplate, input {TemplateName required}, output {Template}, single modeled error TemplateDoesNotExistException -> awsQuery code `TemplateDoesNotExist`, HTTP 400) answers 200 with Template.TemplateName == <n> and the SubjectPart/HtmlPart/TextPart EXACTLY as declared, placeholders unrendered (the `{{name}}` handlebars text is stored verbatim, never substituted at rest). TestRenderTemplate(TemplateName=<n>, TemplateData='{"name": "Ada"}') (Smithy: com.amazonaws.ses#TestRenderTemplate, input {TemplateName required, TemplateData required @length 0..262144}, errors InvalidRenderingParameter/MissingRenderingAttribute/TemplateDoesNotExist, all HTTP 400) then substitutes the declared variable into the rendered MIME output, and TemplateData that omits a declared variable is refused HTTP 400 code `MissingRenderingAttribute` with the message "Attribute 'name' is not present in the rendering data." — fixed text "Attribute '" + the missing variable name taken from the template's declared placeholders + "' is not present in the rendering data." Deleting the stack removes the underlying template, after which GetTemplate answers TemplateDoesNotExist.

Emulator: cfn.create_stack is accepted and the `stack_create_complete` waiter returns, but the SES template was never provisioned. The very first read after the deploy is refused: GetTemplate answers HTTP 400, code `TemplateDoesNotExist`, message "Template <name> does not exist." — naming the exact TemplateName the stack's template parameter declared. So the deployment reports success while its resource is absent. (The per-run generated template name is a value, not a requirement; the requirement is that the declared name is resolvable after CREATE_COMPLETE.)

Why: a CREATE_COMPLETE that provisions nothing is the most misleading outcome available to a client. Every infrastructure-as-code workflow that deploys an SES template and then sends templated mail through it (SendTemplatedEmail / SendBulkTemplatedEmail) breaks one call later with a not-found error that points at the template rather than at the deployment; a caller cannot distinguish "my template body was wrong" from "the stack silently created nothing", and stack deletion cannot be verified to remove anything.

Related but NOT covering: the open issue 20261026T065304Z-ses-6-net-new-behavioral-root-causes-sesv2.md records the same class of defect (Cause 6) for AWS::SES::ConfigurationSet, AWS::SES::ReceiptRuleSet and AWS::SES::ReceiptRule only — its fix text and its Issue-Trace-Ids name those three resource types and do not include AWS::SES::Template or this check's replay key. 20261026T062103Z-codecommit-2-net-new-behavioral-root-causes-a.md records it for AWS::CodeCommit::Repository and 20261026T023803Z-apigatewayv2-cloudformation-stack-outputs-carry-no-outputvalue-and.md for AWS::ApiGatewayV2::DomainName; neither covers SES.

Fix: the CloudFormation resource-provisioning layer (the cloudformation crate's resource-type provider registry) must materialise `AWS::SES::Template` into the ses crate's template state on stack create — writing TemplateName/SubjectPart/HtmlPart/TextPart through the same state writer CreateTemplate uses, so GetTemplate resolves it — and remove it on stack delete, rather than recording only the stack resource. Note for the fixer: once provisioning lands, this check continues into TestRenderTemplate, which is a separate unimplemented scaffold stub at services/service-com-amazonaws-ses-simpleemailservice/src/ops/test_render_template.rs already filed by the replay report (it blocks 6 checks in this suite); that stub must also be implemented, including the MissingRenderingAttribute refusal above, before this check passes end to end.

Traces: ses.deploy-ses-template-cfn-renders.

## Evidence
Cause 1: expected, after cfn.create_stack(TemplateBody=<AWSTemplateFormatVersion 2010-09-09 template with Resources.SesTemplate of Type AWS::SES::Template, Properties.Template.TemplateName = !Ref TemplateNameParam, SubjectPart 'Welcome {{name}}', HtmlPart '<h1>Hello {{name}}</h1>...', TextPart 'Hello {{name}}, ...'>) and the stack_create_complete waiter returning: ses.get_template(TemplateName=<declared name>) -> HTTP 200 with Template.TemplateName == the declared name, '{{name}}' still present in SubjectPart and HtmlPart; then ses.test_render_template(TemplateName=<n>, TemplateData='{"name": "Ada"}') containing 'Welcome Ada' and 'Hello Ada' with no '{{name}}' left, and ses.test_render_template(TemplateName=<n>, TemplateData='{}') refused ('MissingRenderingAttribute', "Attribute 'name' is not present in the rendering data.").
Actual — the stack reached CREATE_COMPLETE (the waiter returned, no CFN error) and the next call was refused: botocore.errorfactory.TemplateDoesNotExistException: An error occurred (TemplateDoesNotExist) when calling the GetTemplate operation: Template sim2real-ses-cfntpl-7be059e36d36 does not exist. (HTTP 400; `sim2real-ses-cfntpl-7be059e36d36` is this run's generated name — the requirement is that whatever name the template declared is resolvable, not that literal value). Verdict: [error] ses.deploy-ses-template-cfn-renders.

## Steps to Reproduce
1. Boot the emulator and point boto3 at it with clients 'ses' (awsQuery v1) and 'cloudformation'.
2. Pick a template name N and deploy: cfn.create_stack(StackName=S, TemplateBody=<YAML declaring Parameters.TemplateNameParam (String) and Resources.SesTemplate of Type AWS::SES::Template with Properties.Template = {TemplateName: !Ref TemplateNameParam, SubjectPart: 'Welcome {{name}}', HtmlPart: '<h1>Hello {{name}}</h1><p>Thanks for joining, {{name}}.</p>', TextPart: 'Hello {{name}}, thanks for joining.'}>, Parameters=[{'ParameterKey':'TemplateNameParam','ParameterValue':N}]); cfn.get_waiter('stack_create_complete').wait(StackName=S) -> observe it returns successfully.
3. ses.get_template(TemplateName=N) -> observe HTTP 400 TemplateDoesNotExist 'Template N does not exist.' instead of HTTP 200 with the declared SubjectPart/HtmlPart/TextPart and the '{{name}}' placeholders intact.
4. After the provisioning fix, also check ses.test_render_template(TemplateName=N, TemplateData='{"name": "Ada"}') renders 'Welcome Ada'/'Hello Ada' with no '{{name}}' remaining, and that TemplateData='{}' is refused ('MissingRenderingAttribute', "Attribute 'name' is not present in the rendering data.").
5. cfn.delete_stack(StackName=S); wait stack_delete_complete; ses.get_template(TemplateName=N) must then be refused TemplateDoesNotExist.
6. Re-replay the check listed in trace_ids.
