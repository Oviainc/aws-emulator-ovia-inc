Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessrepo-application-version-publish
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessrepo-application-version-publish serverlessrepo-cloudformation-template-generation
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.version
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.applicationId
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.creationTime
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.parameterDefinitions
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.requiredCapabilities
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.resourcesSupported
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.semanticVersion
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|$.templateUrl
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->4xx|status
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.version
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.version.semanticVersion
Issue-Key: serverlessrepo|ListApplicationVersions|2xx->4xx|$.versions[*].semanticVersion
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->4xx|$.errorCode
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->4xx|$.message
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->4xx|status
Issue-Related: 20261022T011815Z-serverlessrepo-createapplication-rejects-a-valid-author-so-the.md
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# serverlessrepo version publish and template generation fixtures 404 after CreateApplication rejection

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
CreateApplication with an inline SAM template rejects the request, so the whole version-publish surface (CreateApplicationVersion, GetApplication with a version, ListApplicationVersions and the version conflict and template-validation error paths) is unreachable and answers 404. This bundle carries 7 causes; the first is the root cause and the other six are the distinct operations it leaves broken. All seven must be closed before submitting. The same Author-pattern root cause is also reported for the CRUD lifecycle in the companion issue "serverlessrepo CreateApplication rejects a valid Author so the application CRUD lifecycle 404s" - that issue does not cover the causes below, which are separate operations with their own replay steps.

Cause 1 of 7 - CreateApplication with TemplateBody is rejected (fixture serverlessrepo-application-version-publish, step setup#1).
AWS: Smithy: CreateApplication POST /applications, success code 201; CreateApplicationRequest.Author is plain __string with no @pattern or @length in the on-disk model; CreateApplicationResponse carries a Version member of shape Version whose ApplicationId, SemanticVersion, TemplateUrl, CreationTime, ParameterDefinitions, RequiredCapabilities and ResourcesSupported are all required. Recorded behavior: creating an application with an inline SAM template returns 201 with the full application body plus the first version, including parameterDefinitions derived from the template's Parameters and resourcesSupported true.
Emulator: returns 400; applicationId, name, description, author, isVerifiedAuthor, labels, creationTime and the whole version member are absent. Example message: the Author-pattern rejection described in the companion issue.
Why: a client cannot publish an application with its first version in one call, which is the documented CreateApplication flow.
Fix: CreateApplication's input validation (drop the unmodeled Author pattern) and its handler (build and return the initial Version from the supplied template).

Cause 2 of 7 - GetApplication cannot read back the newly published application (step setup#2).
AWS: Smithy: GetApplication GET /applications/{ApplicationId}, success code 200. Recorded behavior: the application created by setup#1 is immediately readable and returns 200.
Emulator: returns 404 NotFoundException on all 15 poll attempts.
Why: read-after-create is broken, so no client can verify a publish.
Fix: follows from Cause 1 - CreateApplication must persist the application.

Cause 3 of 7 - CreateApplicationVersion cannot publish a second semantic version (step task#3).
AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success code 201; output carries ApplicationId, SemanticVersion, TemplateUrl, CreationTime, ParameterDefinitions, RequiredCapabilities and ResourcesSupported. Recorded behavior: publishing a new version returns 201 with those members populated, requiredCapabilities an empty list and resourcesSupported true.
Emulator: returns 404; every output member is absent.
Why: version publication, the core SAR workflow, cannot be exercised at all.
Fix: follows from Cause 1; CreateApplicationVersion's handler must resolve the application and create the version.

Cause 4 of 7 - GetApplication does not report the application's current version (step task#4).
AWS: Smithy: GetApplicationResponse.Version targets the Version shape (jsonName "version"). Recorded behavior: GetApplication returns 200 with the version member describing the application's default semantic version.
Emulator: returns 404; the poll on the version's semanticVersion exhausts 15 attempts with the member absent.
Why: a client cannot discover which version an application currently resolves to.
Fix: follows from Cause 1; GetApplication must serialize the stored default Version.

Cause 5 of 7 - ListApplicationVersions does not list published versions (step task#5).
AWS: Smithy: ListApplicationVersions GET /applications/{ApplicationId}/versions, success code 200, output member Versions. Recorded behavior: returns 200 with one VersionSummary per published semantic version, each carrying applicationId, semanticVersion and creationTime, ordered ascending by semantic version.
Emulator: returns 404; the versions member is absent and the poll for the newest semantic version exhausts 15 attempts.
Why: a client enumerating an application's versions cannot page them.
Fix: follows from Causes 1 and 3; ListApplicationVersions must return the persisted version summaries.

Cause 6 of 7 - CreateApplicationVersion does not report a duplicate version as a conflict (step task#7).
AWS: Smithy: CreateApplicationVersion errors include ConflictException with httpError 409. Recorded behavior: re-publishing an existing semantic version returns 409 with errorCode "VersionAlreadyExistsException" (example message: "Cannot publish version <v> for application <arn> because it already exists").
Emulator: returns 404 with x-amzn-errortype NotFoundException and errorCode "404".
Why: a client that treats 409/VersionAlreadyExistsException as an idempotent re-publish sees a not-found instead and will retry or fail hard.
Fix: follows from Cause 1; CreateApplicationVersion's handler must check for an existing semantic version and raise ConflictException (409).

Cause 7 of 7 - CreateApplicationVersion does not validate that the template is a SAM template (step task#8).
AWS: Smithy: CreateApplicationVersion errors include BadRequestException with httpError 400. Recorded behavior: a CloudFormation template that has no Transform field is rejected with 400 and errorCode "InvalidRequestException" (example message: "Resource with name [template] is invalid. Resource must have Transform field: ...").
Emulator: returns 404 with x-amzn-errortype NotFoundException and errorCode "404" - the application lookup fails before any template validation runs.
Why: a client submitting a plain CloudFormation template gets a not-found rather than the validation error that tells it what to fix.
Fix: follows from Cause 1, then CreateApplicationVersion's template validation must require the SAM Transform field and raise BadRequestException (400).

## Evidence
Cause 1 (serverlessrepo-application-version-publish#setup#1, CreateApplication): expected status 201, got 400. Absent: $.applicationId, $.name, $.description, $.author, $.isVerifiedAuthor, $.labels, $.creationTime, $.version (expected the full Version object including parameterDefinitions [{name QueueVisibility, type Number, defaultValue 30, referencedByResources [Q]}], requiredCapabilities [], resourcesSupported true). Cluster also covers serverlessrepo-cloudformation-template-generation#setup#1.

Cause 2 (#setup#2, GetApplication): expected 200, got 404; poll: status 404 not in [200] after 15 attempt(s); body {'__type':'NotFoundException','errorCode':'404','message':'No application exists with the id <arn>.'}. Cluster also covers serverlessrepo-cloudformation-template-generation#setup#2.

Cause 3 (#task#3, CreateApplicationVersion): expected 201, got 404; absent: $.applicationId, $.semanticVersion, $.templateUrl, $.creationTime, $.parameterDefinitions, $.requiredCapabilities, $.resourcesSupported.

Cause 4 (#task#4, GetApplication): expected 200, got 404; $.version absent; $.version.semanticVersion poll exhausted after 15 attempt(s).

Cause 5 (#task#5, ListApplicationVersions): expected 200, got 404; $.versions absent (expected two VersionSummary entries); $.versions[1].semanticVersion poll exhausted after 15 attempt(s).

Cause 6 (#task#7, CreateApplicationVersion): expected 409, got 404; $.errorCode expected 'VersionAlreadyExistsException', got '404'; $headers.x-amzn-errortype got 'NotFoundException'.

Cause 7 (#task#8, CreateApplicationVersion): expected 400, got 404; $.errorCode expected 'InvalidRequestException', got '404'; $headers.x-amzn-errortype got 'NotFoundException'.

## Steps to Reproduce
1. Replay fixture serverlessrepo-application-version-publish against the emulator.
2. Step setup#1 issues POST /applications with templateBody and an Author containing a space; observe 400 instead of 201 with the application plus its first version.
3. Step setup#2 polls GET /applications/{id}; observe 404 on all 15 attempts.
4. Step task#3 issues PUT /applications/{id}/versions/{semanticVersion}; observe 404 instead of 201.
5. Steps task#4 and task#5 issue GET /applications/{id} and GET /applications/{id}/versions; observe 404 instead of 200 with the version / versions members.
6. Step task#7 re-PUTs the same semantic version; observe 404 / errorCode '404' instead of 409 / errorCode 'VersionAlreadyExistsException'.
7. Step task#8 PUTs a version whose template has no Transform field; observe 404 / errorCode '404' instead of 400 / errorCode 'InvalidRequestException'.
