Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessrepo-application-version-publish
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessrepo-application-version-publish serverlessapplicationrepository-application-lifecycle serverlessapplicationrepository-application-policy serverlessapplicationrepository-application-version-lifecycle serverlessapplicationrepository-cloudformation-template-preparation serverlessrepo-cloudformation-template-generation
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$.errorCode
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$.message
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|status
Issue-Key: serverlessrepo|DeleteApplication|2xx->2xx|$headers.content-length
Issue-Key: serverlessrepo|GetCloudFormationTemplate|4xx->4xx|$.message
Issue-Commit: d5912a3a34a2e2faa23a606635408f9fea83c969

# serverlessrepo protocol gaps: 204 omits Content-Length, 404 template message reworded, transform-less template not refused

> _Note: this issue was identified against the code base at commit `d5912a3a34a2e2faa23a606635408f9fea83c969`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
The three causes below are serverlessrepo HTTP-protocol divergences: a request AWS refuses that the emulator accepts, an error message that does not read as AWS words it, and a success response that omits a header AWS always sends. They share no handler, but they are all wire-level envelope/refusal faults on one service. This bundle carries 3 CAUSES and ALL 3 must be closed before submitting; the two trace tokens listed with the issue are the replay units for causes 2 and 3.

Related but NOT covering: the open behavioral issue "serverlessrepo template handling is JSON-only: a YAML SAM template yields no ParameterDefinitions and its missing Transform is never refused" (20261022T182143Z-serverlessapplicationrepository-serverlessrepo-template-handling-is-json-only-a-yaml.md) names the same reader as cause 1, but it is behavioral and carries no wire fixture/step, so its replay never exercises this recorded step (the replay reports it unowned) and it states the refusal as `BadRequestException` whereas the RECORDING shows the body's `errorCode` is `InvalidRequestException`. Also related and not covering: 20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md.

Cause 1 of 3 — CreateApplicationVersion ACCEPTS a template with no SAM `Transform` instead of refusing it with 400 (fixture serverlessrepo-application-version-publish, step task#8).
AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success code 201, with service-common error BadRequestException (errorKind client, httpError 400, members Message/jsonName `message` and ErrorCode/jsonName `errorCode`). Recorded behavior: publishing a plain CloudFormation template that carries no `Transform` field is REFUSED with HTTP 400, a JSON body carrying `errorCode` "InvalidRequestException" and `message` of the form "Resource with name [template] is invalid. Resource must have Transform field: <the parsed template document>", plus an `x-amzn-errortype` response header (shape-asserted). Fixed text of the message: "Resource with name [" + the offending resource/field name + "] is invalid. Resource must have Transform field: "; the variable tail is the submitted template as the service parsed it, so the recording's rendered document is a value, not part of the template.
Emulator: answers 201 and PUBLISHES the version — no `$.errorCode`, no `$.message`, no `x-amzn-errortype`. The Transform check is reached only for bodies that parse as JSON, so a YAML template is never judged.
Why: a client that submits raw CloudFormation instead of a packaged SAM template gets a published version that cannot be deployed, instead of the 400 that tells it to add the Transform; the application's version set then contains a version AWS would never have created, and error-handling paths that depend on the refusal never run.
Fix: the template reader behind the publish paths — `parse_template` in src/state.rs and its callers in src/ops/create_application_version.rs and the CreateApplication handler — must require the SAM `Transform` field for YAML templates as well as JSON ones and raise the modelled 400 (body `errorCode` as recorded, `x-amzn-errortype` header set) while storing nothing.
Traces: serverlessrepo-application-version-publish#task#8.

Cause 2 of 3 — GetCloudFormationTemplate's 404 message is not worded as AWS words it (fixture serverlessapplicationrepository-cloudformation-template-preparation, step task#4).
AWS: Smithy: GetCloudFormationTemplate GET /applications/{ApplicationId}/templates/{TemplateId}; error NotFoundException (errorKind client, httpError 404, Message jsonName `message`, ErrorCode jsonName `errorCode`). Recorded behavior: an unknown TemplateId is refused with HTTP 404 and `message` "Template with id <TemplateId> can not be found." — fixed text "Template with id " + " can not be found." with exactly one variable part, the TemplateId taken from the request URI; the message names NO application.
Emulator: returns the right status (404) but the message "Template with id <TemplateId> could not be found for application <ApplicationId ARN>." — it substitutes "could not be found for application <arn>." for AWS's "can not be found." and appends the application ARN, which AWS's message does not mention.
Why: an error message is part of the contract: clients and test suites that match on SAR's wording (and humans reading logs) see text AWS never emits, and the extra ARN leaks a different resource into the message.
Fix: the NotFoundException construction in the GetCloudFormationTemplate handler (src/ops/get_cloud_formation_template.rs) — emit AWS's fixed text with the request's TemplateId as its only interpolation.
Traces: serverlessapplicationrepository-cloudformation-template-preparation#task#4 (cluster also covers serverlessrepo-cloudformation-template-generation#task#5).

Cause 3 of 3 — DeleteApplication's 204 omits the `Content-Length: 0` header (fixture serverlessapplicationrepository-application-lifecycle, step task#6).
AWS: Smithy: DeleteApplication DELETE /applications/{ApplicationId}, output Unit, success code 204 (restJson1, so no body). Recorded behavior: the 204 response carries `content-length: 0` — a fixed, request-independent value.
Emulator: sends the 204 with NO `content-length` header at all (replay: `$headers.content-length` missing, expected '0').
Whip through the consequence — Why: an HTTP client or proxy that distinguishes "explicitly empty" from "length unknown" (connection-reuse logic, strict SDK response readers, recording proxies) handles the emulator's 204 differently from AWS's, and the divergence is visible on every delete.
Fix: the DeleteApplication handler / the empty-body serializer path for Unit-output operations (src/ops/delete_application.rs and the shared 204 response construction) must set `Content-Length: 0` on the no-body success response.
Traces: serverlessapplicationrepository-application-lifecycle#task#6 (cluster also covers serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9, serverlessapplicationrepository-cloudformation-template-preparation#task#6).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 3 unowned mismatch clusters.

Cause 1 — serverlessrepo-application-version-publish#task#8, CreateApplicationVersion: expected status 400, got 201. `$.message`: missing (expected "Resource with name [template] is invalid. Resource must have Transform field: OrderedDict({'AWSTemplateFormatVersion': '2010-09-09', 'Resources': OrderedDict({'Q': OrderedDict({'Type': 'AWS::SQS::Queue'})})})", got None); `$.errorCode`: missing (expected 'InvalidRequestException', got None); `$headers.x-amzn-errortype`: missing (shaped expectation, got None). The rendered template in the expected message is the recording's own submitted document — a value filling the template, not fixed text.

Cause 2 — serverlessapplicationrepository-cloudformation-template-preparation#task#4, GetCloudFormationTemplate: status 404 expected and 404 received; `$.message`: value_mismatch (expected 'Template with id 00000000-0000-0000-0000-000000000000 can not be found.', got 'Template with id 00000000-0000-0000-0000-000000000000 could not be found for application arn:aws:serverlessrepo:us-west-2:000000000000:applications/<app-name>.'). The template id is the one the request asked for; the ARN appears only in the emulator's text.

Cause 3 — serverlessapplicationrepository-application-lifecycle#task#6, DeleteApplication: status 204 expected and 204 received; `$headers.content-length`: missing (expected '0', got None). Same cluster at serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9, serverlessapplicationrepository-cloudformation-template-preparation#task#6 — 4 affected steps.

Code: services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/state.rs — `parse_template` (~line 1737) returns `Ok(Vec::new())` for any body that is not JSON, before the Transform check, so a YAML body is never judged (cause 1).

## Steps to Reproduce
1. Replay fixture serverlessrepo-application-version-publish step task#8: CreateApplicationVersion on an existing application with a YAML template that is byte-identical to the accepted control template except for the missing `Transform: AWS::Serverless-2016-10-31` line. Observe 201 and a published version instead of HTTP 400 with body `errorCode` 'InvalidRequestException', the "Resource with name [template] is invalid. Resource must have Transform field: ..." message, and an `x-amzn-errortype` header.
2. Replay fixture serverlessapplicationrepository-cloudformation-template-preparation step task#4: GetCloudFormationTemplate with a TemplateId that does not exist for the application. Observe the 404 message "Template with id <id> could not be found for application <arn>." instead of "Template with id <id> can not be found.".
3. Replay fixture serverlessapplicationrepository-application-lifecycle step task#6 (DeleteApplication) and inspect the 204 response headers: no `content-length` header is sent, where AWS sends `content-length: 0`. Reproduces on every DeleteApplication (also at serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9, serverlessapplicationrepository-cloudformation-template-preparation#task#6).
