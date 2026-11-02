Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-sap.get-operation-valid-uuid-not-found-rejected ssm-sap.register-application-duplicate-id-conflict-rejected ssm-sap.put-resource-permission-invalid-action-type-rejected ssm-sap.resource-permission-lifecycle-get-after-delete ssm-sap.tag-resource-lifecycle-and-nonexistent-arn
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap.get-operation-valid-uuid-not-found-rejected|run#7|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: ssm-sap.register-application-duplicate-id-conflict-rejected|run#13|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: ssm-sap.put-resource-permission-invalid-action-type-rejected|run#11|AssertionError|assert err_missing["Code"] == "ValidationException", err_missing
Issue-Key: ssm-sap.resource-permission-lifecycle-get-after-delete|run#11|AssertionError|assert err_get["Code"] == "ValidationException", err_get
Issue-Key: ssm-sap.tag-resource-lifecycle-and-nonexistent-arn|run#14|AssertionError|assert err["Code"] == "NotFoundException", err
Issue-Commit: 1f4ddd4c47b0507bb1e50344cb9c3bb105203334

# ssm-sap not-found error families and messages diverge: GetOperation/RegisterApplication wordings are the emulator's own, and the resource-permission and tagging operations answer the wrong error code entirely

> _Note: this issue was identified against the code base at commit `1f4ddd4c47b0507bb1e50344cb9c3bb105203334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePermission

## Description
Four distinct root causes in how ssm-sap reports a reference it cannot resolve. In two of them the error CODE is wrong (so the client's error branch is wrong, not just its log line); in two the code is right and the message text is the emulator's own invention. ssm-sap deliberately uses DIFFERENT error families for the same logical "doesn't exist" condition on different operations, so per-operation wording and code are the only signal a client has.

Cause 1 of 4 — GetOperation's not-found message is the emulator's own wording.
AWS: GetOperation with a well-formed UUID that names no operation is refused with ValidationException (HTTP 400) and the message "Operation Not Found: <OperationId>" — fixed text "Operation Not Found: " followed by the OperationId taken verbatim from the request. Smithy: GetOperation declares only InternalServerException, so ValidationException arrives as the service-common error (ValidationException, httpError 400, "The input fails to satisfy the constraints specified by an AWS service.").
Emulator: code ValidationException is right, but the message is "No operation with ID <OperationId> exists in this account and Region" — different fixed text around the same interpolated id.
Why: a client (or a test) that matches GetOperation's message to tell "no such operation" apart from the other ValidationException causes of this operation fails to classify the error, and message assertions against the emulator are not portable to AWS.
Fix: the not-found message helper used by the GetOperation handler (src/ops/get_operation.rs and the message builders in src/state.rs).
Traces: ssm-sap.get-operation-valid-uuid-not-found-rejected.

Cause 2 of 4 — RegisterApplication's duplicate-ApplicationId conflict message is the emulator's own wording.
AWS: registering a second application under an ApplicationId that already exists is refused with ConflictException (HTTP 409) and the message "Application id '<ApplicationId>' already exists." — fixed text "Application id '" / "' already exists." with the ApplicationId from the request inside single quotes. Smithy: RegisterApplication errors include ConflictException (httpError 409, "A conflict has occurred.").
Emulator: code ConflictException is right, message is "An application with ID <ApplicationId> is already registered in this account and Region" — different fixed text, and it drops the quoting AWS uses around the id.
Why: idempotent-register logic that keys on the conflict message (the code alone does not distinguish a duplicate id from the other conflicts RegisterApplication can raise) misclassifies the failure.
Fix: the conflict message helper used by the RegisterApplication handler (src/ops/register_application.rs and src/state.rs).
Traces: ssm-sap.register-application-duplicate-id-conflict-rejected.

Cause 3 of 4 — the resource-permission operations answer ResourceNotFoundException where AWS answers ValidationException, with a different message.
AWS: PutResourcePermission, GetResourcePermission and DeleteResourcePermission are database-scoped: the ResourceArn must name an actual, discovered SAP HANA database, not merely match the ARN pattern. An ARN that is pattern-valid but names no discovered database — including the ARN of a real, registered APPLICATION, which is not a database ARN — is refused with ValidationException (HTTP 400). The message is "Database ARN '<arn>' is not valid" for GetResourcePermission/DeleteResourcePermission, and "ResourceArn: Database ARN '<arn>' is not valid" for PutResourcePermission (the member-name prefix "ResourceArn: " is fixed text identifying the offending input member); <arn> is the ARN from the request, in single quotes. Separately, an ActionType outside the modeled enum is refused with ValidationException and the standard constraint message "1 validation error detected: Value '<value>' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [RESTORE]" — fixed text with <value> from the request. Smithy: PermissionActionType enum values = [RESTORE]; PutResourcePermission/GetResourcePermission/DeleteResourcePermission declare InternalServerException + ResourceNotFoundException, with ValidationException as the service-common error — so the model does admit ResourceNotFoundException for these ops, but the recorded behavior for an unresolvable database ARN is ValidationException 400, not 404.
Emulator: all three operations answer ResourceNotFoundException (HTTP 404) with the message "Resource <arn> doesn't exist." — wrong error code, wrong HTTP status, and a message that neither quotes the ARN nor mentions that the ARN must be a DATABASE ARN. Because the emulator never reaches the ActionType enum case (the resource check refuses first), the enum-constraint message above is also unverified for this path.
Why: a client distinguishing "your ARN is not a database ARN" (a 400 it must fix in the request) from "the resource is gone" (a 404 it may treat as already-deleted) takes the wrong branch; retry/`ignore-if-absent` logic keyed on 404 silently swallows a malformed request, and SDK retry classification differs between 400 and 404.
Fix: the ResourceArn resolution in the three resource-permission handlers (src/ops/put_resource_permission.rs, src/ops/get_resource_permission.rs, src/ops/delete_resource_permission.rs) — they must resolve the ARN as a discovered HANA database and raise ValidationException with the AWS wording, instead of falling back on the generic application-not-found path in src/state.rs.
Traces: ssm-sap.put-resource-permission-invalid-action-type-rejected, ssm-sap.resource-permission-lifecycle-get-after-delete.

Cause 4 of 4 — TagResource on a nonexistent resource ARN answers the wrong error code and message.
AWS: TagResource with a syntactically valid but nonexistent application ARN is refused with error code NotFoundException and the message "Resource was not found" — entirely fixed text, with no part of the ARN interpolated. The real application's tags are untouched by the rejected call. Recorded behavior only for the code/message: the Smithy model declares TagResource errors as ConflictException and ResourceNotFoundException, so the wire code AWS actually returns (NotFoundException) is narrower than, and textually different from, the modeled shape name.
Emulator: TagResource answers code ResourceNotFoundException with message "Resource <arn> doesn't exist." — a different error code and a different message that interpolates the ARN.
Why: a client catching NotFoundException on TagResource (the code botocore exposes for the real service) does not catch the emulator's ResourceNotFoundException at all, so the exception escapes the handler; message-based assertions also fail.
Fix: the not-found path of the TagResource handler (src/ops/tag_resource.rs) — it must raise the NotFoundException code with AWS's fixed message rather than reusing the shared application-not-found message helper in src/state.rs. ListTagsForResource/UntagResource, which share the same lookup, should be checked for the same divergence.
Traces: ssm-sap.tag-resource-lifecycle-and-nonexistent-arn.

## Evidence
Replay output, per cause.

Cause 1 — ssm-sap.get-operation-valid-uuid-not-found-rejected:
  AssertionError: expected message 'Operation Not Found: ef3dabaa-b0d6-445e-ae78-e9a29f8d81f4', got 'No operation with ID ef3dabaa-b0d6-445e-ae78-e9a29f8d81f4 exists in this account and Region'
  (Code ValidationException matched; only the message text diverges. The UUID is a per-run value; the divergence is in the fixed text around it.)

Cause 2 — ssm-sap.register-application-duplicate-id-conflict-rejected:
  AssertionError: expected message "Application id 'sim2real-sap-498461c4a720' already exists.", got 'An application with ID sim2real-sap-498461c4a720 is already registered in this account and Region'
  (Code ConflictException matched; the application id is a per-run value, the fixed text and the quoting diverge.)

Cause 3 — ssm-sap.put-resource-permission-invalid-action-type-rejected:
  expected: Code ValidationException (400), Message "ResourceArn: Database ARN '<arn>' is not valid"
  actual:   {'Message': "Resource arn:aws:ssm-sap:us-west-2:000000000000:application/sim2real-sap-never-7c1bbd43acce doesn't exist.", 'Code': 'ResourceNotFoundException'}
  ssm-sap.resource-permission-lifecycle-get-after-delete (GetResourcePermission; DeleteResourcePermission and the real-application-ARN positive control in the same check are on the same path):
  expected: Code ValidationException (400), Message "Database ARN '<arn>' is not valid"
  actual:   {'Message': "Resource arn:aws:ssm-sap:us-west-2:000000000000:application/sim2real-sap-never-3cc47beb0f75 doesn't exist.", 'Code': 'ResourceNotFoundException'}
  Also unverified behind this refusal: ActionType='NOT_A_REAL_ACTION_TYPE' must give ValidationException "1 validation error detected: Value 'NOT_A_REAL_ACTION_TYPE' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [RESTORE]" (Smithy PermissionActionType values = [RESTORE]).

Cause 4 — ssm-sap.tag-resource-lifecycle-and-nonexistent-arn:
  expected: Code 'NotFoundException', Message 'Resource was not found'
  actual:   {'Message': "Resource arn:aws:ssm-sap:us-west-2:000000000000:application/does-not-exist-sim2real-sap-5fd15563d1a0 doesn't exist.", 'Code': 'ResourceNotFoundException'}
  (The TagResource/ListTagsForResource/UntagResource round-trip on a real ARN earlier in the check passed.)

## Steps to Reproduce
Cause 1: POST /get-operation {"OperationId":"<any fresh UUID v4>"} -> observe 400 ValidationException with message 'No operation with ID <uuid> exists in this account and Region'; AWS answers 'Operation Not Found: <uuid>'.

Cause 2: POST /register-application {"ApplicationId":"DUP1","ApplicationType":"HANA","Instances":["i-0deadbeefdeadbeef"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"CredentialType":"ADMIN","DatabaseName":"HDB/HDB","SecretId":"any"}]}; repeat the identical call with a different instance id -> observe 409 ConflictException with message 'An application with ID DUP1 is already registered in this account and Region'; AWS answers "Application id 'DUP1' already exists."

Cause 3: register application REAL1 as above and take its Arn A. Build N = A with the last ARN path segment replaced by an id that was never registered.
  a) POST /put-resource-permission {"ActionType":"RESTORE","SourceResourceArn":A,"ResourceArn":N} -> observe 404 ResourceNotFoundException "Resource N doesn't exist."; AWS answers 400 ValidationException "ResourceArn: Database ARN 'N' is not valid".
  b) POST /get-resource-permission {"ResourceArn":N} and POST /delete-resource-permission {"ResourceArn":N} -> same 404; AWS answers 400 ValidationException "Database ARN 'N' is not valid".
  c) POST /get-resource-permission {"ResourceArn":A} (a real APPLICATION ARN, not a database ARN) -> AWS also answers 400 ValidationException "Database ARN 'A' is not valid".
  d) POST /put-resource-permission {"ActionType":"NOT_A_REAL_ACTION_TYPE","SourceResourceArn":A,"ResourceArn":A} -> AWS answers 400 ValidationException "1 validation error detected: Value 'NOT_A_REAL_ACTION_TYPE' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [RESTORE]".

Cause 4: register application REAL2 and take its Arn A. POST /tags/{resourceArn} with resourceArn = A with the last path segment prefixed by 'does-not-exist-', body {"tags":{"x":"y"}} -> observe code ResourceNotFoundException with message "Resource <bogus-arn> doesn't exist."; AWS answers code NotFoundException with message 'Resource was not found'. Then GET /tags/{A} and confirm the real application's tags are unchanged (they are, in both).
