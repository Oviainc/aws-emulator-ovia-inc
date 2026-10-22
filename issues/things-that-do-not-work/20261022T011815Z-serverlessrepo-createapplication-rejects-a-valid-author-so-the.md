Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessrepo-application-crud-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessrepo-application-crud-lifecycle serverlessrepo-application-policy-sharing serverlessrepo-application-version-publish serverlessrepo-cloudformation-template-generation
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.applicationId
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.author
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.creationTime
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.description
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.isVerifiedAuthor
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.labels
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|$.name
Issue-Key: serverlessrepo|CreateApplication|2xx->4xx|status
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.applicationId
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.author
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.creationTime
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.description
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.isVerifiedAuthor
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.labels
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.name
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|status
Issue-Key: serverlessrepo|GetApplication|2xx->4xx|$.homePageUrl
Issue-Key: serverlessrepo|ListApplicationVersions|2xx->4xx|$.versions
Issue-Key: serverlessrepo|ListApplicationVersions|2xx->4xx|status
Issue-Key: serverlessrepo|CreateApplication|4xx->4xx|$.errorCode
Issue-Key: serverlessrepo|CreateApplication|4xx->4xx|$.message
Issue-Key: serverlessrepo|CreateApplication|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: serverlessrepo|CreateApplication|4xx->4xx|status
Issue-Key: serverlessrepo|DeleteApplication|2xx->4xx|status
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# serverlessrepo CreateApplication rejects a valid Author so the application CRUD lifecycle 404s

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
The emulator applies an Author input pattern that AWS does not apply, so CreateApplication fails outright and every subsequent step of the application CRUD lifecycle operates on an application that was never created. This bundle carries 6 causes; the first is the root cause and the other five are the distinct downstream operations it breaks. All six must be closed before submitting.

Cause 1 of 6 - CreateApplication rejects an Author containing a space (fixture serverlessrepo-application-crud-lifecycle, step setup#1).
AWS: Smithy: CreateApplication POST /applications, success code 201; CreateApplicationRequest.Author targets __string and carries NO @pattern and NO @length constraint in the on-disk model, and the modeled client errors are BadRequestException (400) and ConflictException (409). Recorded behavior: an Author value containing a space is accepted, the call returns 201, and the body carries applicationId, name, description, author, isVerifiedAuthor, labels and creationTime.
Emulator: returns 400 with x-amzn-errortype BadRequestException and no success body at all - applicationId, name, description, author, isVerifiedAuthor, labels and creationTime are all absent. Example message: "Author must satisfy pattern ^[a-z0-9](([a-z0-9]|-(?!-))*[a-z0-9])?$: sim2real probe". Hypothesis: the lowercase-hyphen pattern AWS applies to the application NAME has been attached to the Author member as well.
Why: a client cannot create an application with a human-readable author string, which is the normal case. The operation is unusable and nothing downstream of it can run.
Fix: CreateApplication's input validation - remove the unmodeled pattern constraint from the Author member.

Cause 2 of 6 - CreateApplication does not detect a duplicate application name (step task#8).
AWS: Smithy: CreateApplication errors include ConflictException with httpError 409. Recorded behavior: re-creating an application with a name that already exists returns 409 with errorCode "ApplicationAlreadyExistsException" (example message: "Application with id <application-arn> already exists.").
Emulator: returns 400 with x-amzn-errortype BadRequestException and errorCode "400", carrying the Author-pattern message instead. Because Cause 1 rejects the request before any name-uniqueness check, the conflict path is never reached.
Why: a client that relies on 409/ConflictException to detect an already-published application sees a validation error instead and cannot distinguish "already exists" from "bad input".
Fix: CreateApplication's handler - perform the name-uniqueness check and raise ConflictException (409).

Cause 3 of 6 - GetApplication cannot read back the created application (step task#2).
AWS: Smithy: GetApplication GET /applications/{ApplicationId}, success code 200, errors include NotFoundException (404). Recorded behavior: the application created by setup is readable and returns 200 with applicationId, name, description, author, isVerifiedAuthor, labels and creationTime.
Emulator: returns 404 NotFoundException on every one of 15 poll attempts; no success body members are present.
Why: read-after-create does not work, so any client that creates then reads an application fails.
Fix: follows from Cause 1 - CreateApplication must persist the application into state.

Cause 4 of 6 - GetApplication does not reflect the UpdateApplication metadata rewrite (step task#4).
AWS: Smithy: UpdateApplication PATCH /applications/{ApplicationId}, success code 200, with optional Author, Description, HomePageUrl and Labels members. Recorded behavior: after the rewrite, GetApplication returns 200 with the new description, the new author, the rewritten labels list and homePageUrl.
Emulator: returns 404; the poll on the rewritten author value exhausts 15 attempts with the member absent.
Why: a client cannot confirm that a metadata update landed.
Fix: follows from Cause 1, then UpdateApplication's handler must apply the patch to the stored application.

Cause 5 of 6 - ListApplicationVersions 404s instead of returning an empty version list (step task#5).
AWS: Smithy: ListApplicationVersions GET /applications/{ApplicationId}/versions, success code 200, output member Versions (jsonName "versions"). Recorded behavior: an application created with no version returns 200 with versions as an empty list.
Emulator: returns 404 and the versions member is absent.
Why: a client enumerating versions of a version-less application gets an error rather than an empty page.
Fix: follows from Cause 1; ListApplicationVersions must return 200 with an empty Versions list for a known application.

Cause 6 of 6 - DeleteApplication 404s during teardown (step teardown#9).
AWS: Smithy: DeleteApplication DELETE /applications/{ApplicationId}, success code 204, errors include NotFoundException (404). Recorded behavior: deleting the application created by setup returns 204.
Emulator: returns 404.
Why: teardown of a created application fails, leaking resources in a client's cleanup path.
Fix: follows from Cause 1; DeleteApplication must find and remove the persisted application.

## Evidence
Cause 1 (serverlessrepo-application-crud-lifecycle#setup#1, CreateApplication): expected status 201, got 400. Missing from body: $.applicationId, $.name, $.description, $.author, $.isVerifiedAuthor, $.labels, $.creationTime. Emulator message example: 'Author must satisfy pattern ^[a-z0-9](([a-z0-9]|-(?!-))*[a-z0-9])?$: sim2real probe'. The on-disk Smithy model declares CreateApplicationRequest.Author as plain __string with no pattern. Cluster also covers serverlessrepo-application-policy-sharing#setup#1.

Cause 2 (#task#8, CreateApplication): expected status 409, got 400. $.errorCode expected 'ApplicationAlreadyExistsException', got '400'. $headers.x-amzn-errortype expected the modeled conflict error type, got 'BadRequestException'.

Cause 3 (#task#2, GetApplication): expected status 200, got 404; poll: status 404 not in [200] after 15 attempt(s); actual body {'__type': 'NotFoundException', 'errorCode': '404', 'message': 'No application exists with the id <application-arn>.'}.

Cause 4 (#task#4, GetApplication): expected status 200, got 404; $.author poll: rewritten author not observed after 15 attempt(s); $.description, $.labels, $.homePageUrl all absent.

Cause 5 (#task#5, ListApplicationVersions): expected status 200, got 404; $.versions expected [], got None.

Cause 6 (#teardown#9, DeleteApplication): expected status 204, got 404. Cluster also covers serverlessrepo-application-version-publish#teardown#9 and serverlessrepo-cloudformation-template-generation#teardown#7.

## Steps to Reproduce
1. Replay fixture serverlessrepo-application-crud-lifecycle against the emulator.
2. Step setup#1 issues POST /applications with an Author value containing a space; observe 400 BadRequestException naming an Author pattern instead of 201 with the application body.
3. Steps task#2, task#4, task#5 and teardown#9 then issue GET /applications/{id}, PATCH + GET /applications/{id}, GET /applications/{id}/versions and DELETE /applications/{id}; each returns 404 NotFoundException because setup#1 persisted nothing.
4. Step task#8 re-issues POST /applications with the same name; observe 400 with the Author-pattern message instead of 409 ConflictException / errorCode ApplicationAlreadyExistsException.
