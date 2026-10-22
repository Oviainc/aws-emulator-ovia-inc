Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-application-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-application-lifecycle serverlessapplicationrepository-application-version-lifecycle serverlessrepo-application-crud-lifecycle
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|GetApplication|4xx->4xx|$.errorCode
Issue-Key: serverlessrepo|GetApplication|4xx->4xx|$.message
Issue-Related: 20261022T011815Z-serverlessrepo-createapplication-rejects-a-valid-author-so-the.md
Issue-Related: 20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# serverlessrepo error responses put the HTTP status in errorCode instead of the error name

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Every serverlessrepo error body serializes the numeric HTTP status into the errorCode member instead of the specific SAR error code name. The HTTP status and the x-amzn-errortype header are right; the errorCode field in the JSON body - which is a modeled response member and therefore a wire requirement - is wrong on every error path exercised. This bundle carries 4 causes, one per distinct error path. All four must be closed before submitting.

Shared AWS grounding: Smithy: the ConflictException, NotFoundException and BadRequestException shapes each declare members ErrorCode (jsonName "errorCode") and Message (jsonName "message"), with httpError 409, 404 and 400 respectively. The model's member documentation for ErrorCode is only the bare status number, so the authoritative source for its value is the recording: AWS populates errorCode with the specific SAR error-code NAME, never the numeric status. Shared emulator behavior: errorCode is set to the HTTP status rendered as a string.
Shared Why: a client that branches on the errorCode field - the only place SAR distinguishes ApplicationAlreadyExists from VersionAlreadyExists, both of which are 409 ConflictException - cannot tell the two apart and will mis-handle the conflict. A client matching on the error name sees no match at all.
Shared Fix: the restJson1 error serializer for this service - populate the ErrorCode member with the SAR error-code name for the raised condition rather than the response status.

Cause 1 of 4 - CreateApplication duplicate name (fixture serverlessapplicationrepository-application-lifecycle, step task#5).
AWS: 409 with errorCode "ApplicationAlreadyExistsException"; the recorded message is an example ("Application with id <arn> already exists.").
Emulator: status 409 is correct; errorCode is "409".

Cause 2 of 4 - GetApplication on a deleted application (fixture serverlessapplicationrepository-application-lifecycle, step task#7).
AWS: 404 with errorCode "ApplicationNotFoundException"; the recorded message is an example ("Application with id <arn> could not be found.").
Emulator: status 404 is correct; errorCode is "404".

Cause 3 of 4 - CreateApplicationVersion duplicate semantic version (fixture serverlessapplicationrepository-application-version-lifecycle, step task#3).
AWS: 409 with errorCode "VersionAlreadyExistsException"; recorded message is an example. Note this is the SAME status and the SAME modeled ConflictException as Cause 1, so errorCode is the only field that separates them.
Emulator: status 409 is correct; errorCode is "409".

Cause 4 of 4 - CreateApplicationVersion with a non-semver version string (fixture serverlessapplicationrepository-application-version-lifecycle, step task#4).
AWS: 400 with errorCode "InvalidRequestException". The message rule here is known - it is produced by semantic-version validation of the request and AWS states it referring the caller to semver.org - so a message naming the semver requirement is required, though the exact recorded wording is only an example.
Emulator: status 400 is correct; errorCode is "400". Its message does name SemanticVersion and the semver.org requirement, so only errorCode is defective on this path.

## Evidence
Cause 1 (serverlessapplicationrepository-application-lifecycle#task#5, CreateApplication, status 409 expected and returned): $.errorCode expected 'ApplicationAlreadyExistsException', got '409'. $.message expected (example) 'Application with id arn:aws:serverlessrepo:us-west-2:000000000000:applications/<name> already exists.', got 'An application with the name <name> already exists.'

Cause 2 (serverlessapplicationrepository-application-lifecycle#task#7, GetApplication, status 404 expected and returned): $.errorCode expected 'ApplicationNotFoundException', got '404'. $.message expected (example) 'Application with id <arn> could not be found.', got 'No application exists with the id <arn>.' Cluster also covers serverlessapplicationrepository-application-version-lifecycle#task#8 and serverlessrepo-application-crud-lifecycle#task#7.

Cause 3 (serverlessapplicationrepository-application-version-lifecycle#task#3, CreateApplicationVersion, status 409 expected and returned): $.errorCode expected 'VersionAlreadyExistsException', got '409'. $.message expected (example) 'Cannot publish version 1.0.0 for application <arn> because it already exists', got 'Version 1.0.0 already exists for application <arn>.'

Cause 4 (serverlessapplicationrepository-application-version-lifecycle#task#4, CreateApplicationVersion, status 400 expected and returned): $.errorCode expected 'InvalidRequestException', got '400'. $.message expected (example) 'Version name is a not valid semantic version. See https://semver.org/ for syntax', got 'SemanticVersion must be a semantic version (https://semver.org/): 1.0'.

Smithy grounding: ConflictException/NotFoundException/BadRequestException each declare ErrorCode (jsonName 'errorCode') and Message (jsonName 'message'); httpError 409/404/400.

## Steps to Reproduce
1. Replay fixtures serverlessapplicationrepository-application-lifecycle and serverlessapplicationrepository-application-version-lifecycle.
2. At application-lifecycle#task#5, POST /applications with a name that already exists; read the 409 body and observe "errorCode": "409" instead of "ApplicationAlreadyExistsException".
3. At application-lifecycle#task#7, GET /applications/{id} for a deleted application; read the 404 body and observe "errorCode": "404" instead of "ApplicationNotFoundException".
4. At application-version-lifecycle#task#3, PUT an already-published semantic version; read the 409 body and observe "errorCode": "409" instead of "VersionAlreadyExistsException".
5. At application-version-lifecycle#task#4, PUT a version whose semantic version is not valid semver; read the 400 body and observe "errorCode": "400" instead of "InvalidRequestException".
