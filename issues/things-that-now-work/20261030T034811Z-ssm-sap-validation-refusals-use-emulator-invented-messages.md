Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-resource-permission-roundtrip-missing-resource
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-resource-permission-roundtrip-missing-resource ssm-sap-application-control-errors-on-missing-app ssm-sap-application-lookup-validation-errors ssm-sap-operation-inspection-validation-errors
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|PutResourcePermission|4xx->4xx|$.Message
Issue-Key: ssm-sap|PutResourcePermission|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-sap|PutResourcePermission|4xx->4xx|status
Issue-Key: ssm-sap|GetResourcePermission|4xx->4xx|$.Message
Issue-Key: ssm-sap|GetResourcePermission|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-sap|GetResourcePermission|4xx->4xx|status
Issue-Key: ssm-sap|DeleteResourcePermission|4xx->4xx|$.Message
Issue-Key: ssm-sap|DeleteResourcePermission|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-sap|DeleteResourcePermission|4xx->4xx|status
Issue-Key: ssm-sap|UpdateApplicationSettings|4xx->4xx|$.Message
Issue-Key: ssm-sap|GetDatabase|4xx->4xx|$.Message
Issue-Key: ssm-sap|ListOperations|4xx->4xx|$.Message
Issue-Commit: 9aa3e2b9dc38edbf8faf15afd04e4b717b6bfa72

# ssm-sap validation refusals use emulator-invented messages and answer ResourceNotFoundException 404 where AWS answers ValidationException 400

> _Note: this issue was identified against the code base at commit `9aa3e2b9dc38edbf8faf15afd04e4b717b6bfa72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePermission

## Description
Six root causes in how ssm-sap refuses a request whose input it cannot resolve or cannot act on. In three of them the HTTP status AND the wire error code are wrong (so a client takes the wrong error branch, not merely logs a different line); in three the code is right and the message text is the emulator's own invention. ssm-sap uses ValidationException (400) for conditions other services report as not-found, so the per-operation code and wording are the only signal a client has.

Related open issue (does NOT cover these wire steps; it is grounded in behavioral check ids and carries no replay key for this corpus): 20261029T184631Z-ssm-sap-not-found-error-families-and-messages.md, whose Cause 3 describes the same resource-permission family defect. Causes 4-6 below must still be closed against the fixture steps named here.

Cause 1 of 6: UpdateApplicationSettings with no updatable member set.
AWS: UpdateApplicationSettings takes an ApplicationId plus at least one update member (CredentialsToAddOrUpdate, CredentialsToRemove, Backint, DatabaseArn). A call that names only ApplicationId is refused with ValidationException (HTTP 400) and the message 'No update option was specified.' — the message is entirely fixed text, with nothing interpolated from the request. Smithy: UpdateApplicationSettings input requires only ApplicationId, the four update members are optional, and errors include ValidationException (httpError 400, 'The input fails to satisfy the constraints specified by an AWS service.'); the no-update-option rule itself is recorded behavior only.
Emulator: status 400 is right, but the message is the emulator's own enumeration of member names, 'backint, credentialsToAddOrUpdate, credentialsToRemove or databaseArn need to be specified'.
Why: a client that distinguishes 'you sent an empty update' from the other ValidationException causes of this operation by message cannot classify the failure, and any assertion written against the emulator's text does not hold on AWS.
Fix: the input-completeness check in the UpdateApplicationSettings handler's validation, and the message helper it calls.
Traces: ssm-sap-application-control-errors-on-missing-app#task#4.

Cause 2 of 6: GetDatabase with an under-specified database selector.
AWS: GetDatabase identifies a database either by DatabaseArn alone or by the full ApplicationId+ComponentId+DatabaseId triple. A call that supplies neither complete alternative is refused with ValidationException (HTTP 400) and the message 'Either database ARN or a combination of applicationId, componentId and databaseId must be provided' — entirely fixed text, nothing interpolated. Smithy: GetDatabase input members ApplicationId, ComponentId, DatabaseId, DatabaseArn are all optional (so the either/or rule is enforced at runtime, recorded behavior only) and the modeled client error is ValidationException (httpError 400).
Emulator: status 400 is right, message is 'databaseArn or databaseId need to be specified' — different fixed text, and it names only two of the four members, so it does not state that ComponentId and ApplicationId are part of the alternative.
Why: the emulator's wording tells a client it may retry with DatabaseId alone, which AWS also refuses; a caller following the error message loops on a second 400.
Fix: the selector-resolution branch of the GetDatabase handler's validation and its message helper.
Traces: ssm-sap-application-lookup-validation-errors#task#4.

Cause 3 of 6: ListOperations for an ApplicationId that names no registered application.
AWS: ListOperations requires ApplicationId; an id that is pattern-valid but names no registered application is refused with ValidationException (HTTP 400) and the message "The application '<ApplicationId>' does not exist" — fixed text "The application '" / "' does not exist" with the ApplicationId copied from the request inside single quotes, and no account id or Region in the text. Smithy: ListOperations declares only InternalServerException, with ValidationException arriving as the service-common client error (httpError 400).
Emulator: status 400 is right, message is 'No registered application found for account: <account-id>, and application: <ApplicationId>' — different fixed text, no quoting, and it leaks the account id into the message.
Why: a client or test that matches ListOperations' not-found text (the code alone does not separate it from the other ValidationException causes) misclassifies the failure; and the emulator's message varies with the account the call was signed for, so it is not even stable across environments.
Fix: the application lookup in the ListOperations handler must raise the application-does-not-exist message in AWS's wording rather than reuse the shared account-scoped not-found message helper in the service state module.
Traces: ssm-sap-operation-inspection-validation-errors#task#7.

Cause 4 of 6: PutResourcePermission with a ResourceArn that resolves to no discovered database.
AWS: the resource-permission operations are database-scoped — ResourceArn must name an actual discovered SAP HANA database, not merely match the ARN pattern. A pattern-valid ARN that names no such database is refused with ValidationException (HTTP 400), x-amzn-errortype: ValidationException, and the message "ResourceArn: Database ARN '<arn>' is not valid" — fixed text "ResourceArn: Database ARN '" / "' is not valid", where the "ResourceArn: " prefix is the fixed member-name tag identifying the offending input member and <arn> is the ARN from the request in single quotes. Smithy: PutResourcePermission input requires ActionType/SourceResourceArn/ResourceArn (Arn pattern ^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$) and declares InternalServerException, ResourceNotFoundException (httpError 404) and ValidationException (httpError 400) — the model admits a 404, but the recorded behavior for an unresolvable database ARN is the 400 ValidationException.
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and the message "Resource <arn> doesn't exist." — wrong status, wrong error code, and a message that neither quotes the ARN nor says the ARN must be a DATABASE ARN.
Why: a client separating 'this is not a database ARN, fix the request' (a 400 it must not retry) from 'the resource is gone' (a 404 that idempotent delete/ignore-if-absent logic swallows) takes the wrong branch, and SDK retry classification differs between the two statuses.
Fix: the ResourceArn resolution in the PutResourcePermission handler — resolve the ARN as a discovered HANA database and raise ValidationException with AWS's wording instead of falling through to the generic resource-not-found path in the service state module.
Traces: ssm-sap-resource-permission-roundtrip-missing-resource#task#1.

Cause 5 of 6: GetResourcePermission with a ResourceArn that resolves to no discovered database.
AWS: same rule as Cause 4, but without the member-name prefix: ValidationException, HTTP 400, x-amzn-errortype: ValidationException, message "Database ARN '<arn>' is not valid" — fixed text "Database ARN '" / "' is not valid" with the request's ARN in single quotes. Smithy: GetResourcePermission requires ResourceArn and declares InternalServerException, ResourceNotFoundException (404) and ValidationException (400).
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Resource <arn> doesn't exist.".
Why: as Cause 4 — a get-before-use probe reads the 404 as 'no permission yet' and proceeds, when AWS is telling it the ARN it holds is not a database ARN at all.
Fix: the ResourceArn resolution in the GetResourcePermission handler, same seam as Cause 4.
Traces: ssm-sap-resource-permission-roundtrip-missing-resource#task#2.

Cause 6 of 6: DeleteResourcePermission with a ResourceArn that resolves to no discovered database.
AWS: same as Cause 5 — ValidationException, HTTP 400, x-amzn-errortype: ValidationException, message "Database ARN '<arn>' is not valid" (fixed text, request ARN quoted). Smithy: DeleteResourcePermission requires ResourceArn and declares InternalServerException, ResourceNotFoundException (404) and ValidationException (400).
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Resource <arn> doesn't exist.".
Why: cleanup code that treats 404 as 'already deleted' silently accepts a malformed request that AWS rejects, so the teardown reports success without having deleted anything.
Fix: the ResourceArn resolution in the DeleteResourcePermission handler, same seam as Causes 4-5.
Traces: ssm-sap-resource-permission-roundtrip-missing-resource#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200), per cause.

Cause 1 — ssm-sap-application-control-errors-on-missing-app#task#4, UpdateApplicationSettings, status 400 expected and 400 observed:
  $.Message: expected 'No update option was specified.', got 'backint, credentialsToAddOrUpdate, credentialsToRemove or databaseArn need to be specified'.

Cause 2 — ssm-sap-application-lookup-validation-errors#task#4, GetDatabase, status 400 expected and 400 observed:
  $.Message: expected 'Either database ARN or a combination of applicationId, componentId and databaseId must be provided', got 'databaseArn or databaseId need to be specified'.

Cause 3 — ssm-sap-operation-inspection-validation-errors#task#7, ListOperations, status 400 expected and 400 observed:
  $.Message: expected "The application '<ApplicationId>' does not exist", got 'No registered application found for account: <account-id>, and application: <ApplicationId>'. The recorded run used ApplicationId sim2real-probe-ssm-sap-ops-1bf35815 and account 000000000000; both are per-run/per-environment values — the divergence is the fixed text, the missing single quotes and the leaked account id.

Cause 4 — ssm-sap-resource-permission-roundtrip-missing-resource#task#1, PutResourcePermission:
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.Message: expected "ResourceArn: Database ARN '<arn>' is not valid", got "Resource <arn> doesn't exist." (recorded ARN arn:aws:ssm-sap:us-west-2:000000000000:database/sim2real-probe-ssm-sap-perm-c9f334c6 — a per-run value that fills the template).

Cause 5 — ssm-sap-resource-permission-roundtrip-missing-resource#task#2, GetResourcePermission:
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.Message: expected "Database ARN '<arn>' is not valid", got "Resource <arn> doesn't exist.".

Cause 6 — ssm-sap-resource-permission-roundtrip-missing-resource#task#3, DeleteResourcePermission:
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.Message: expected "Database ARN '<arn>' is not valid", got "Resource <arn> doesn't exist.".

## Steps to Reproduce
Cause 1: POST /update-application-settings {"ApplicationId":"<any id, registered or not>"} with no other member -> observe 400 with Message 'backint, credentialsToAddOrUpdate, credentialsToRemove or databaseArn need to be specified'; AWS answers 400 ValidationException 'No update option was specified.'

Cause 2: POST /get-database {"DatabaseId":"HDB"} (no DatabaseArn, no ApplicationId/ComponentId) -> observe 400 'databaseArn or databaseId need to be specified'; AWS answers 400 ValidationException 'Either database ARN or a combination of applicationId, componentId and databaseId must be provided'.

Cause 3: POST /list-operations {"ApplicationId":"<id that was never registered>"} -> observe 400 'No registered application found for account: <account>, and application: <id>'; AWS answers 400 ValidationException "The application '<id>' does not exist".

Causes 4-6: build D = arn:aws:ssm-sap:<region>:<account>:database/<id-that-was-never-discovered>.
  a) POST /put-resource-permission {"ActionType":"RESTORE","SourceResourceArn":D,"ResourceArn":D} -> observe 404 / x-amzn-errortype ResourceNotFoundException / "Resource D doesn't exist."; AWS answers 400 / ValidationException / "ResourceArn: Database ARN 'D' is not valid".
  b) POST /get-resource-permission {"ResourceArn":D} -> same 404; AWS answers 400 ValidationException "Database ARN 'D' is not valid".
  c) POST /delete-resource-permission {"ResourceArn":D} -> same 404; AWS answers 400 ValidationException "Database ARN 'D' is not valid".
Then re-replay fixture ssm-sap-resource-permission-roundtrip-missing-resource steps task#1, task#2 and task#3, plus ssm-sap-application-control-errors-on-missing-app#task#4, ssm-sap-application-lookup-validation-errors#task#4 and ssm-sap-operation-inspection-validation-errors#task#7.
