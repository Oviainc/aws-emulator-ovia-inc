Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-application-policy
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-application-policy serverlessapplicationrepository-application-version-lifecycle serverlessapplicationrepository-cloudformation-template-preparation serverlessapplicationrepository-application-lifecycle
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplication|2xx->2xx|$.labels
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.labels
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.version.templateUrl
Issue-Key: serverlessrepo|CreateApplication|2xx->2xx|$.version.templateUrl
Issue-Key: serverlessrepo|DeleteApplication|2xx->2xx|$headers.content-length
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->2xx|$.templateUrl
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# serverlessrepo response serialization drops empty labels, presigned templateUrl and 204 Content-Length

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
The serverlessrepo response serializer omits members and framing that AWS always emits: an empty Labels list is dropped from the body entirely, the Version.TemplateUrl it emits is not a presigned URL, and the 204 DeleteApplication response omits Content-Length. The statuses are all correct on these paths. This bundle carries 5 causes. All five must be closed before submitting.

Cause 1 of 5 - CreateApplication omits labels when the application has no labels (fixture serverlessapplicationrepository-application-policy, step task#1).
AWS: Smithy: CreateApplicationResponse.Labels targets __listOf__string with jsonName "labels". Recorded behavior: when an application is created with no labels, the 201 body still carries "labels": [] - the member is present and empty, not omitted.
Emulator: returns 201 (correct) but the labels member is absent from the body.
Why: a client that reads response["labels"] without a presence guard raises a KeyError or NoneType error on an application that simply has no labels; a client diffing an application's labels cannot distinguish "no labels" from "not reported".
Fix: CreateApplication's response serializer - always serialize the Labels member, emitting an empty list rather than omitting it.

Cause 2 of 5 - GetApplication omits labels the same way (fixture serverlessapplicationrepository-application-version-lifecycle, step task#7).
AWS: Smithy: GetApplicationResponse.Labels, jsonName "labels". Recorded behavior: the 200 body carries "labels": [] for a label-less application.
Emulator: returns 200 (correct) with the labels member absent.
Why: same as Cause 1, on the read path - the most common place a client inspects labels.
Fix: GetApplication's response serializer.

Cause 3 of 5 - CreateApplication with an inline template omits labels (fixture serverlessapplicationrepository-cloudformation-template-preparation, step task#1).
AWS: as Cause 1. Recorded behavior: the 201 body carries "labels": [] alongside the nested version object.
Emulator: returns 201 (correct) with labels absent.
Why: same as Cause 1; confirms the omission is not specific to the no-template create path.
Fix: shared with Cause 1 - the Application serializer used by both CreateApplication shapes.

Cause 4 of 5 - DeleteApplication's 204 response omits the Content-Length header (fixture serverlessapplicationrepository-application-lifecycle, step task#6).
AWS: Smithy: DeleteApplication DELETE /applications/{ApplicationId} with success code 204 and an empty (Unit) output. Recorded behavior: the 204 response carries Content-Length: 0.
Emulator: returns 204 (correct) but sends no Content-Length header at all.
Why: an HTTP client or proxy that requires explicit framing on an empty response may hold the connection open waiting for a body or treat the response as unframed; a client asserting on response headers sees a missing header.
Fix: DeleteApplication's response serializer - emit Content-Length: 0 on the empty 204.

Cause 5 of 5 - Version.TemplateUrl is not a presigned URL (fixture serverlessapplicationrepository-application-version-lifecycle, step task#2).
AWS: Smithy: Version.TemplateUrl (jsonName "templateUrl") is a required member documented as "A link to the packaged AWS SAM template of your application". Recorded behavior only: the recording asserts the SHAPE of this URL rather than its literal value, and AWS's value is a presigned S3 URL carrying the X-Amz-* signature query parameters that make it fetchable without credentials.
Emulator: returns 201 (correct) with a bare, unsigned S3 object URL that fails the recorded shape assertion. The host, account id and region segments of the URL are environment-dependent and are NOT the requirement here - the missing presigning query string is.
Why: a client that GETs templateUrl to download the packaged SAM template receives an access-denied rather than the template, because the URL carries no signature.
Fix: the Version serializer / template storage for CreateApplicationVersion and CreateApplication - generate a presigned URL for the stored template. The same defect is visible on GetApplication's nested version member (see Traces).

## Evidence
Cause 1 (serverlessapplicationrepository-application-policy#task#1, CreateApplication, status 201 expected and returned): $.labels missing (expected [], got None). Cluster also covers serverlessapplicationrepository-application-version-lifecycle#task#1.

Cause 2 (serverlessapplicationrepository-application-version-lifecycle#task#7, GetApplication, status 200 expected and returned): $.labels missing (expected [], got None); $.version.templateUrl shape assertion failed (expected '<shaped-not-asserted>', got an unsigned 'https://awsserverlessrepo-<region>.s3.<region>.amazonaws.com/<account>/<app>/<version>/template.yaml').

Cause 3 (serverlessapplicationrepository-cloudformation-template-preparation#task#1, CreateApplication, status 201 expected and returned): $.labels missing (expected [], got None); $.version.templateUrl shape assertion failed the same way.

Cause 4 (serverlessapplicationrepository-application-lifecycle#task#6, DeleteApplication, status 204 expected and returned): $headers.content-length missing (expected '0', got None). Cluster also covers serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9 and serverlessapplicationrepository-cloudformation-template-preparation#task#6.

Cause 5 (serverlessapplicationrepository-application-version-lifecycle#task#2, CreateApplicationVersion, status 201 expected and returned): $.templateUrl expected '<shaped-not-asserted>' (a presigned URL shape), got an unsigned S3 object URL.

Smithy grounding: CreateApplicationResponse.Labels / GetApplicationResponse.Labels target __listOf__string with jsonName 'labels'; DeleteApplication has success code 204 with Unit output; Version.TemplateUrl is a required member.

## Steps to Reproduce
1. Replay fixtures serverlessapplicationrepository-application-policy, serverlessapplicationrepository-application-version-lifecycle, serverlessapplicationrepository-cloudformation-template-preparation and serverlessapplicationrepository-application-lifecycle.
2. At application-policy#task#1, POST /applications with no labels member; the 201 body omits "labels" instead of carrying "labels": [].
3. At application-version-lifecycle#task#7, GET /applications/{id}; the 200 body omits "labels".
4. At cloudformation-template-preparation#task#1, POST /applications with templateBody; the 201 body omits "labels".
5. At application-lifecycle#task#6, DELETE /applications/{id}; the 204 response carries no Content-Length header.
6. At application-version-lifecycle#task#2, PUT /applications/{id}/versions/{v}; the returned templateUrl is a bare S3 URL with no X-Amz-* presigning query parameters.
