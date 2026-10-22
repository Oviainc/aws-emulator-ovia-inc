Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-application-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-application-lifecycle serverlessapplicationrepository-application-version-lifecycle
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.description
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.labels[*]
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$.errorCode
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$.message
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->2xx|status
Issue-Key: serverlessrepo|ListApplicationVersions|2xx->2xx|$.versions[*]
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# serverlessrepo application mutations unapplied: UpdateApplication no-op and unvalidated version publish

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApplication

## Description
On the fixtures where an application IS created successfully, two state mutations are handled wrongly: UpdateApplication reports success but does not persist the change, and CreateApplicationVersion publishes a version whose template AWS rejects, after which the bogus version is listed. This bundle carries 3 causes. All three must be closed before submitting.

Cause 1 of 3 - UpdateApplication does not persist the description or the new label (fixture serverlessapplicationrepository-application-lifecycle, step task#4).
AWS: Smithy: UpdateApplication PATCH /applications/{ApplicationId}, success code 200, with optional Description (jsonName "description") and Labels (jsonName "labels") members; GetApplication GET /applications/{ApplicationId} returns the same Description and Labels members. Recorded behavior: after a PATCH that rewrites the description and appends a label, the subsequent GetApplication returns 200 with the UPDATED description and the label list including the appended entry.
Emulator: returns 200 on the GetApplication (correct status) but serves the pre-update values - the description is still the original string and the appended label is absent from the labels list.
Why: a client that patches application metadata gets a success response for a write that silently did not happen, and any read-back or convergence loop sees stale data forever. This is a lost-write, the most damaging class of state defect.
Fix: UpdateApplication's handler/state - apply the Description and Labels members of the patch to the stored application instead of discarding them.

Cause 2 of 3 - CreateApplicationVersion accepts a CloudFormation template that has no SAM Transform field (fixture serverlessapplicationrepository-application-version-lifecycle, step task#5).
AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success code 201, errors include BadRequestException with httpError 400. Recorded behavior: a template body that is valid CloudFormation but carries no Transform field is rejected with 400, errorCode "InvalidRequestException" and an x-amzn-errortype header; the message rule is known - it is built from the request and names the offending resource and the missing Transform field (example: "Resource with name [template] is invalid. Resource must have Transform field: ...").
Emulator: returns 201 and creates the version. No error body is produced at all - message, errorCode and x-amzn-errortype are all absent.
Why: the emulator accepts an application version that AWS refuses, so a client's SAM template validation passes locally and then fails against real AWS - exactly the class of bug an emulator exists to catch. There is no way for a client to test its rejection path.
Fix: CreateApplicationVersion's validation - require the SAM Transform field in the supplied template and raise BadRequestException (400) with errorCode InvalidRequestException when it is missing.

Cause 3 of 3 - the version wrongly accepted in Cause 2 is then returned by ListApplicationVersions (fixture serverlessapplicationrepository-application-version-lifecycle, step task#6).
AWS: Smithy: ListApplicationVersions GET /applications/{ApplicationId}/versions, success code 200, output member Versions (jsonName "versions"). Recorded behavior: because the Transform-less publish was rejected, the version list contains only the legitimately published version - the rejected semantic version never appears.
Emulator: returns 200 (correct status) with an EXTRA entry in versions for the semantic version that should never have been created, carrying applicationId, semanticVersion and creationTime.
Why: the application's version list is corrupted with a version that does not exist on AWS, so a client enumerating versions deploys or resolves one that AWS has no record of.
Fix: follows from Cause 2 - rejecting the invalid publish must leave no version behind in state; verify ListApplicationVersions reads only committed versions.

## Evidence
Cause 1 (serverlessapplicationrepository-application-lifecycle#task#4, GetApplication, status 200 expected and returned): $.description value_mismatch - expected the rewritten description 'sim2real probe application lifecycle updated', got the pre-update 'sim2real probe application lifecycle'. $.labels[2] missing - expected the appended label 'updated', got None. The preceding PATCH /applications/{id} reported success.

Cause 2 (serverlessapplicationrepository-application-version-lifecycle#task#5, CreateApplicationVersion): status expected 400, got 201. $.message missing (expected, as an example of the known rule, "Resource with name [template] is invalid. Resource must have Transform field: OrderedDict({'AWSTemplateFormatVersion': '2010-09-09', 'Resources': OrderedDict({'Q': OrderedDict({'Type': 'AWS::SQS::Queue'})})})"); $.errorCode missing (expected 'InvalidRequestException'); $headers.x-amzn-errortype missing.

Cause 3 (serverlessapplicationrepository-application-version-lifecycle#task#6, ListApplicationVersions, status 200 expected and returned): $.versions[1] extra - expected None, got {'applicationId': '<arn>', 'semanticVersion': '2.0.0', 'creationTime': '<timestamp>'} - the version created by the publish that Cause 2 should have rejected.

Smithy grounding: UpdateApplication PATCH /applications/{ApplicationId} code 200 with optional Description and Labels; CreateApplicationVersion code 201 with BadRequestException httpError 400; ListApplicationVersions code 200 with output member Versions.

## Steps to Reproduce
1. Replay fixtures serverlessapplicationrepository-application-lifecycle and serverlessapplicationrepository-application-version-lifecycle.
2. In application-lifecycle, PATCH /applications/{id} rewriting description and appending a label, then at task#4 GET /applications/{id}; observe the 200 body still carries the original description and lacks the appended label.
3. In application-version-lifecycle at task#5, PUT /applications/{id}/versions/2.0.0 with a templateBody that is plain CloudFormation and has no Transform field; observe 201 instead of 400 / errorCode 'InvalidRequestException'.
4. At task#6, GET /applications/{id}/versions; observe the versions array contains the 2.0.0 entry created in step 3, which should not exist.
