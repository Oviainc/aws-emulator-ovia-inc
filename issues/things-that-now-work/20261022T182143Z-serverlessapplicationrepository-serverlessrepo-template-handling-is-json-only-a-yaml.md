Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: serverlessrepo.application-version-publish-lifecycle serverlessrepo.rejects-template-without-transform
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo.application-version-publish-lifecycle|run#12|AssertionError|assert first["ParameterDefinitions"] == [
Issue-Key: serverlessrepo.rejects-template-without-transform|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Commit: 00577afa9aa0eead33f7dafc8adac2dd3659e1fc

# serverlessrepo template handling is JSON-only: a YAML SAM template yields no ParameterDefinitions and its missing Transform is never refused

> _Note: this issue was identified against the code base at commit `00577afa9aa0eead33f7dafc8adac2dd3659e1fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
Both causes below are observed through CreateApplicationVersion with an inline `TemplateBody`, and both come from the same place: `crate::state::parse_template` only reads a template that parses as JSON (`serde_json::from_str`) and returns `Ok(Vec::new())` for anything else, so every YAML template — the form the SAM CLI, the console and these checks publish — is stored unexamined. This bundle carries 2 causes; BOTH must be closed before submitting. Related but NOT covering: the open wire issue "serverlessrepo version publish and template generation fixtures 404 after CreateApplication rejection" (20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md) describes parameterDefinitions and the Transform refusal as consequences of a CreateApplication 404; here CreateApplication/CreateApplicationVersion SUCCEED and still get both wrong, so that issue's replay steps do not exercise these causes.

Cause 1 of 2 — ParameterDefinitions is not derived from a YAML SAM template.
AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success 201; output member ParameterDefinitions targets __listOfParameterDefinition (jsonName parameterDefinitions), and ParameterDefinition requires Name and ReferencedByResources and carries Type, DefaultValue, Description, Allowed*/Min*/Max*, NoEcho; the same list is required on the Version shape that GetApplication returns. Recorded behavior: for a template declaring `Parameters: {QueueVisibility: {Type: Number, Default: 30}}` referenced by resource `Q` as `!Ref QueueVisibility`, SAR derives exactly one ParameterDefinition — Name QueueVisibility, Type Number, DefaultValue the string '30', ReferencedByResources ['Q'] — and derives it per version from that version's own template (a version whose template declares no Parameters reports an empty list, and publishing a later version must not rewrite an earlier version's derived list).
Emulator: ParameterDefinitions comes back as an empty list for that template; the handler stores whatever `parse_template` returned, which is `Vec::new()` for any body that is not JSON.
Why: a client that reads an application's parameters before deploying it (the SAM/console deploy flow, and anything building a parameter form or validating ParameterOverrides) sees a parameterless application and cannot discover required or defaulted parameters, nor which resources use them.
Fix: the template reader behind CreateApplicationVersion/CreateApplication (crate::state::parse_template and its callers in src/ops/create_application_version.rs and src/ops/create_application.rs) must read YAML SAM templates as well as JSON — including the CloudFormation short forms the reference scan depends on (`!Ref`, `!Sub`) — and derive ParameterDefinitions from the parsed document, per version.

Cause 2 of 2 — a template with no SAM Transform is accepted instead of refused.
AWS: Smithy: CreateApplicationVersion (and CreateApplication) model BadRequestException with httpError 400 ("One of the parameters in the request is invalid", errorCode member documented as 400). Recorded behavior: publishing a plain CloudFormation template that carries no `Transform` field is refused with HTTP 400 / error code BadRequestException and leaves no version behind (ListApplicationVersions still lists only the previously published version); on the create path the same refusal leaves no application at all, so GetApplication on that id answers NotFoundException and the name is still free for a later create. Example message: "Resource with name [template] is invalid. Resource must have Transform field: ..." (the recorded text echoes the parsed template, so it is an example, not the requirement).
Emulator: CreateApplicationVersion with a transform-less YAML template SUCCEEDS (201, a version is published). The Transform check in parse_template is reached only for JSON bodies, so the YAML body is never judged.
Why: a client that submits raw CloudFormation instead of a packaged SAM template gets a published version that cannot be deployed, instead of the 400 that tells it to add the Transform; the application's version set then contains a version AWS would never have created.
Fix: the same template reader — once a YAML template is read, require the SAM `Transform` field and raise BadRequestException (400) on both CreateApplicationVersion and CreateApplication, publishing/storing nothing for the refused request.

Traces: serverlessapplicationrepository.application-version-publish-lifecycle (Cause 1), serverlessapplicationrepository.rejects-template-without-transform (Cause 2). Note that rejects-template-without-transform aborts at the first refusal, so its later claims (the refused create leaves no application and the name is reusable) are unverified rather than passing.

## Evidence
Cause 1 — check serverlessapplicationrepository.application-version-publish-lifecycle: `create_application_version(ApplicationId=..., SemanticVersion='1.0.0', TemplateBody=<YAML SAM template with Parameters.QueueVisibility>)` returned ParameterDefinitions []. AssertionError: "SAR derived ParameterDefinitions []; expected one Number parameter QueueVisibility defaulting to '30' and referenced by resource Q" (expected [{'Name': 'QueueVisibility', 'DefaultValue': '30', 'Type': 'Number', 'ReferencedByResources': ['Q']}]).

Cause 2 — check serverlessapplicationrepository.rejects-template-without-transform: `create_application_version(ApplicationId=<existing app>, SemanticVersion='2.0.0', TemplateBody="AWSTemplateFormatVersion: '2010-09-09'\nResources:\n  Q:\n    Type: AWS::SQS::Queue\n")` returned success. AssertionError: "the call succeeded; real AWS refuses it" — expected a ClientError with Code 'BadRequestException' and HTTP 400. The only difference from the accepted control template is the single `Transform: AWS::Serverless-2016-10-31` line.

Code: services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/state.rs, parse_template — `let Ok(template) = ::serde_json::from_str::<::serde_json::Value>(body) else { return Ok(Vec::new()); };` precedes both the Parameters walk and the `declares_transform` check, so neither runs for a YAML body. Called from src/ops/create_application_version.rs (line ~209) and the CreateApplication handler.

## Steps to Reproduce
1. Replay behavioral check serverlessapplicationrepository.application-version-publish-lifecycle: CreateApplication (no SemanticVersion), then CreateApplicationVersion 1.0.0 with the YAML SAM template declaring Parameters.QueueVisibility (Type Number, Default 30) referenced by resource Q via !Ref; observe ParameterDefinitions [] instead of the one derived Number parameter.
2. Replay behavioral check serverlessapplicationrepository.rejects-template-without-transform: CreateApplication with the transform-bearing YAML template (control, succeeds), then CreateApplicationVersion 2.0.0 with the byte-identical template minus the `Transform: AWS::Serverless-2016-10-31` line; observe 201/success instead of 400 BadRequestException, and a 2.0.0 version appearing in ListApplicationVersions.
3. Repeat step 2 on the create path (CreateApplication with the transform-less template) and observe it is likewise accepted.
