Issue-Source: trace-sweep-divergence
Issue-Trace: medical-imaging-create-datastore-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medical-imaging-create-datastore-validation-errors medical-imaging-list-datastores-validation-errors medical-imaging-notfound-errors-on-missing-datastore
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging|CreateDatastore|4xx->4xx|$.message
Issue-Key: medical-imaging|ListDatastores|4xx->4xx|$.message
Issue-Key: medical-imaging|GetDatastore|4xx->4xx|$.message
Issue-Key: medical-imaging|DeleteDatastore|4xx->4xx|$.message
Issue-Key: medical-imaging|GetDICOMImportJob|4xx->4xx|$.message
Issue-Key: medical-imaging|StartDICOMImportJob|4xx->4xx|$.message
Issue-Commit: 688840645544bc9bcffb33ac47ee3cc89851c839

# medical-imaging error response messages do not follow the service error text contract

> _Note: this issue was identified against the code base at commit `688840645544bc9bcffb33ac47ee3cc89851c839`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatastore

## Description
Across the medical-imaging error paths the emulator returns the right HTTP status and the right error code but composes its own message text instead of the message the service produces. Two distinct rules are broken: (a) the constraint-violation message format, which is derived from the request and therefore a contract, and (b) the service's fixed, resource-independent texts for an invalid pagination token and for a missing datastore (the latter observed identically across four different operations in the recording, so it is a constant of the service, not one recording's value). 6 causes.

Cause 1 of 6: CreateDatastore constraint-violation message format for kmsKeyArn.
AWS: Smithy: CreateDatastore.kmsKeyArn targets KmsKeyArn with @length 1..512 and @pattern ^arn:aws[a-zA-Z-]{0,16}:kms:[a-z]{2}(-[a-z]{1,16}){1,3}-\d{1}:\d{12}:((key/...)|(alias/...))$; the modeled failure is ValidationException (@httpError 400). The message is built from the request, and the recorded rule is '1 validation error detected: <memberName> failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern with the ^ and $ anchors stripped>' — the offending value is NOT echoed and the regex is reported unanchored.
Emulator: HTTP 400 ValidationException, but with the generic format "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: ^...$" — it adds "Value '...' at", and keeps the anchors.
Why: a client or test asserting on the service's documented validation-message shape (including tooling that parses the member name out of it, and anything that must not see the rejected value echoed back) mis-parses the emulator's text; the emulator also reflects a user-supplied value into the error body where the service does not.
Fix: the shared constraint-validation message formatter used by the medical-imaging crate's generated input validation (services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/, CreateDatastore input validation) must emit the member-only, unanchored-pattern form for this service.
Traces: medical-imaging-create-datastore-validation-errors#task#1 (cluster also covers task#2 and task#3 of the same fixture).

Cause 2 of 6: ListDatastores rejection text for a nextToken this service did not issue.
AWS: Smithy: ListDatastores.nextToken (@httpQuery nextToken, NextToken @length 1..8192, @pattern ^\p{ASCII}{0,8192}$), @paginated inputToken=nextToken; modeled failure ValidationException (@httpError 400). Recorded behavior only: HTTP 400 with the fixed text 'Failed to perform requested operation due to invalid NextToken. Please try again.' — the message does not vary with the token supplied.
Emulator: HTTP 400 ValidationException with its own text 'nextToken is not a pagination token issued by this service'.
Why: the status and code are right, so this breaks only message-sensitive clients and recorded-response assertions; it also makes the emulator's pagination failure mode distinguishable from the service's.
Fix: the ListDatastores handler's pagination-token decode path in the medical-imaging crate must emit the service's fixed invalid-NextToken text.
Traces: medical-imaging-list-datastores-validation-errors#task#1.

Cause 3 of 6: GetDatastore not-found text echoes the requested datastore id.
AWS: Smithy: GetDatastore.datastoreId (@httpLabel @required, @pattern ^[0-9a-z]{32}$), modeled ResourceNotFoundException (@httpError 404, 'The request references a resource which does not exist.'). Recorded behavior only, but constant across four operations in this fixture: HTTP 404 with 'Datastore resource is not found.' — resource-independent, with no id in the text.
Emulator: HTTP 404 ResourceNotFoundException with 'Datastore <datastoreId> was not found', i.e. the request's id interpolated into the message.
Why: the emulator's 404 body is id-dependent where the service's is a constant, so message assertions fail and the emulator leaks the probed identifier back to the caller on a not-found.
Fix: the datastore-lookup error path shared by the medical-imaging read/write handlers must raise ResourceNotFoundException with the service's constant text.
Traces: medical-imaging-notfound-errors-on-missing-datastore#task#2.

Cause 4 of 6: DeleteDatastore not-found text echoes the requested datastore id.
AWS: Smithy: DeleteDatastore.datastoreId (@httpLabel @required, ^[0-9a-z]{32}$), modeled ResourceNotFoundException (@httpError 404); recorded text 'Datastore resource is not found.' (same constant).
Emulator: HTTP 404 with 'Datastore <datastoreId> was not found'.
Why: as cause 3, on the delete path.
Fix: same datastore-lookup error path, DeleteDatastore handler.
Traces: medical-imaging-notfound-errors-on-missing-datastore#task#3.

Cause 5 of 6: GetDICOMImportJob not-found text echoes the requested datastore id.
AWS: Smithy: GetDICOMImportJob.datastoreId/jobId (@httpLabel @required), modeled ResourceNotFoundException (@httpError 404); recorded text 'Datastore resource is not found.'
Emulator: HTTP 404 with 'Datastore <datastoreId> was not found'.
Why: as cause 3, on the import-job read path.
Fix: same datastore-lookup error path, GetDICOMImportJob handler.
Traces: medical-imaging-notfound-errors-on-missing-datastore#task#5.

Cause 6 of 6: StartDICOMImportJob not-found text echoes the requested datastore id.
AWS: Smithy: StartDICOMImportJob.datastoreId (@httpLabel @required), modeled ResourceNotFoundException (@httpError 404); recorded text 'Datastore resource is not found.'
Emulator: HTTP 404 with 'Datastore <datastoreId> was not found'.
Why: as cause 3, on the import-job start path.
Fix: same datastore-lookup error path, StartDICOMImportJob handler.
Traces: medical-imaging-notfound-errors-on-missing-datastore#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (statuses and error codes matched; $.message diverged in every case):

medical-imaging-create-datastore-validation-errors#task#1 (CreateDatastore, 400->400, cluster also covers task#2, task#3)
  expected $.message: '1 validation error detected: kmsKeyArn failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-zA-Z-]{0,16}:kms:[a-z]{2}(-[a-z]{1,16}){1,3}-\d{1}:\d{12}:((key/[a-fA-F0-9]{8}-...)|(alias/[a-zA-Z0-9:/_-]{1,256}))'
  actual   $.message: "1 validation error detected: Value 'arn:aws:kms:us-east-1:000000000000:not-a-valid-key-ref' at 'kmsKeyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-zA-Z-]{0,16}:kms:...$"
  -> divergence is structural: the value is echoed and the pattern is reported anchored.

medical-imaging-list-datastores-validation-errors#task#1 (ListDatastores, 400->400)
  expected $.message: 'Failed to perform requested operation due to invalid NextToken. Please try again.'
  actual   $.message: 'nextToken is not a pagination token issued by this service'

medical-imaging-notfound-errors-on-missing-datastore (404->404 in all four steps)
  task#2 GetDatastore        expected 'Datastore resource is not found.' / actual 'Datastore 0f1e2d3c4b5a69788796a5b4c3d2e1f0 was not found'
  task#3 DeleteDatastore     expected 'Datastore resource is not found.' / actual 'Datastore 0f1e2d3c4b5a69788796a5b4c3d2e1f0 was not found'
  task#5 GetDICOMImportJob   expected 'Datastore resource is not found.' / actual 'Datastore 0f1e2d3c4b5a69788796a5b4c3d2e1f0 was not found'
  task#6 StartDICOMImportJob expected 'Datastore resource is not found.' / actual 'Datastore 0f1e2d3c4b5a69788796a5b4c3d2e1f0 was not found'
  -> the identical expected constant on four distinct operations is the evidence that the service text is resource-independent; the id shown above is an example value from the fixture, not a requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay medical-imaging-create-datastore-validation-errors step task#1 — POST /datastore with a clientToken and a kmsKeyArn that violates the KmsKeyArn @pattern; compare $.message against the member-only, unanchored-pattern form.
3. Cause 2: replay medical-imaging-list-datastores-validation-errors step task#1 — GET /datastore?nextToken=<token not issued by the service>; compare $.message against 'Failed to perform requested operation due to invalid NextToken. Please try again.'
4. Causes 3-6: replay medical-imaging-notfound-errors-on-missing-datastore steps task#2, task#3, task#5 and task#6 — GET /datastore/{id}, DELETE /datastore/{id}, GET /getDICOMImportJob/datastore/{id}/job/{jobId} and POST /startDICOMImportJob/datastore/{id} for a well-formed but nonexistent 32-char datastore id; each returns 404 ResourceNotFoundException whose message interpolates the id instead of the service's constant 'Datastore resource is not found.'
