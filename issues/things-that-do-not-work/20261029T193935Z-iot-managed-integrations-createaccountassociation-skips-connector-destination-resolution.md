Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.account-association-lifecycle-and-refresh-conflict iot-managed-integrations.default-encryption-configuration-lifecycle iot-managed-integrations.destination-lifecycle-and-update iot-managed-integrations.managed-thing-lifecycle-state-transitions iot-managed-integrations.notification-configuration-lifecycle iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.account-association-lifecycle-and-refresh-conflict|_error#10|AssertionError|raise AssertionError(f"{what}: expected a ClientError, call succeeded with {result!r}")
Issue-Key: iot-managed-integrations.default-encryption-configuration-lifecycle|run#6|AssertionError|assert code_msg == 'IoT Managed Integrations is not authorized to use the key.', code_msg
Issue-Key: iot-managed-integrations.destination-lifecycle-and-update|run#17|AssertionError|assert code_msg is None, code_msg
Issue-Key: iot-managed-integrations.managed-thing-lifecycle-state-transitions|run#12|AssertionError|assert bad_code_msg == '1 validation error detected: Value at \'authenticationMaterial\' failed to satisfy constraint: Member must satisfy regular expression pa
Issue-Key: iot-managed-integrations.notification-configuration-lifecycle|run#4|AssertionError|assert missing_code_msg == "Destination doesn't exist", missing_code_msg
Issue-Key: iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle|run#4|AssertionError|assert missing_message == "The runtime log configuration is for a managed thing resource that cannot be found.", (
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# iot-managed-integrations: CreateAccountAssociation skips connector-destination resolution, and five refusal bodies diverge from AWS (wrong message text, invented message, anchored regex)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountAssociation

## Description
Six independent root causes in the iot-managed-integrations emulator, all surfaced by the behavioral suite. One is a semantic defect (a reference AWS refuses is accepted and a resource is created); five are refusal-body defects (an error message AWS never words, a message where AWS sends none, and a constraint message that quotes the modelled regex with its anchors). This bundle has 6 causes; ALL SIX must be fixed before it is closed.

Smithy model used throughout: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json

--------------------------------------------------------------------
Cause 1 of 6: CreateAccountAssociation accepts a ConnectorDestinationId that does not exist (or no longer exists) and creates an association.
AWS: Smithy: CreateAccountAssociation, POST /account-associations, success code 201; input member ConnectorDestinationId is @required with @length(1..64) and @pattern ^[A-Za-z0-9-_]+$; modelled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior: when ConnectorDestinationId names a connector destination that is not present, AWS answers HTTP 400 ValidationException with Message 'Connector Destination in the request does not exist.' — that whole sentence is FIXED text for this error class; it names the member's resource class and interpolates NO value from the request (notably it is ValidationException, not the ResourceNotFoundException every other not-found path of this service uses). The recording establishes this for an id that demonstrably existed and was deleted: GetConnectorDestination on that same id already answers 404 ResourceNotFoundException with Message 'Connector Destination not found', so the emulator's own state knows the destination is gone.
Emulator: HTTP 201 with a freshly created association — body carries AccountAssociationId, AssociationState 'ASSOCIATION_IN_PROGRESS', an account-association Arn and an empty OAuthAuthorizationUrl. No ValidationException, no 400.
Why: a client that probes a connector-destination reference before wiring an association is told the association was created. It then builds Get/StartRefresh/Delete flows on a resource real AWS would never have created, and never learns the destination reference is dead — the opposite verdict from AWS, not merely different wording.
Fix: CreateAccountAssociation's request validation (the operation's handler in src/ops/create_account_association.rs) — resolve ConnectorDestinationId against connector-destination state BEFORE allocating the association, and raise ValidationException with AWS's fixed text when it does not resolve. Note that the same check also asserts, after this point, that a SECOND CreateAccountAssociation against a destination with an outstanding association is refused ConflictException 'Account Association conflicts with existing record.' while StartAccountAssociationRefresh on the outstanding one succeeds; those assertions were never reached in this run, so they are not claimed as divergences here, only noted as what the check exercises next.
Same root cause as open issue 20261029T171839Z-iot-managed-integrations-create-operations-skip-referential-validation-and-return-201.md (its Cause 1 of 3), which reached it from the wire corpus; that issue carries no behavioral replay key, so the behavioral check id is recorded here.

--------------------------------------------------------------------
Cause 2 of 6: PutDefaultEncryptionConfiguration refuses CUSTOMER_KEY_ENCRYPTION without a kmsKeyArn using an emulator-authored message.
AWS: Smithy: PutDefaultEncryptionConfiguration, POST /configuration/account/encryption; input encryptionType @required (enum EncryptionType), kmsKeyArn optional with @length(1..200) and @pattern ^arn:aws:kms:[0-9a-zA-Z-]+:[0-9]+:key/[0-9a-zA-Z-]+$; modelled errors include ValidationException (httpError 400, single member Message). The model states no cross-member rule, so: recorded behavior only — AWS answers HTTP 400 ValidationException with Message 'IoT Managed Integrations is not authorized to use the key.' This entire sentence is fixed text; no request value (no ARN, no enum value) is interpolated. Hypothesis (not stated by model or recording): AWS reaches the key-authorization check rather than reporting a missing-member rule.
Emulator: correct code (ValidationException) and status, but Message 'kmsKeyArn is required when encryptionType is "CUSTOMER_KEY_ENCRYPTION"' — wording AWS never emits, and it reports a different rule (missing member) than AWS reports (key not authorized).
Why: a client or suite that matches on, logs or displays AWS's message text sees a different diagnosis; remediation advice derived from the message points at the wrong fix.
Fix: PutDefaultEncryptionConfiguration's validation error construction (src/ops/put_default_encryption_configuration.rs) — emit AWS's fixed text for this refusal.

--------------------------------------------------------------------
Cause 3 of 6: GetDestination's (and UpdateDestination's) ResourceNotFoundException carries a Message where AWS sends none.
AWS: Smithy: GetDestination (GET /destinations/{Name}) and UpdateDestination (PUT /destinations/{Name}) both declare ResourceNotFoundException (httpError 404); every member of that shape (Message, ResourceId, ResourceType) is OPTIONAL. Recorded behavior: for a destination name that is not present, AWS answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and NO Message member at all — the absence of Message is the requirement here, so there is no fixed text and no variable part.
Emulator: HTTP 404 ResourceNotFoundException whose body carries Message 'Destination resource not found.' (observed on the read after DeleteDestination). The check asserts Message is absent on both the read and the post-delete UpdateDestination.
Why: a client that branches on whether Message is present, or that surfaces it, behaves differently against the emulator than against AWS, and the string it reads is an emulator invention.
Fix: the destination operations' error serializer / not-found construction (src/ops/get_destination.rs and src/ops/update_destination.rs) — emit the bare ResourceNotFoundException for these operations.
Same root cause as open issue 20261029T171839Z-iot-managed-integrations-resourcenotfoundexception-bodies-carry-resourceid-resourcetype-and-message-that.md (its Causes 1 and 2), filed from the wire corpus with no behavioral replay key.

--------------------------------------------------------------------
Cause 4 of 6: CreateManagedThing's pattern-constraint message quotes the modelled regex WITH its ^ and $ anchors.
AWS: Smithy: CreateManagedThing input AuthenticationMaterial (target AuthMaterialString) is @required, @sensitive, @length(1..512) and @pattern ^[0-9A-Za-z!#$%&()*\+\-;<=>?@^_`{|}~\/: {},\\"]+$; modelled errors include ValidationException (httpError 400). Recorded behavior: a material containing characters outside that class is refused HTTP 400 ValidationException with the standard AWS constraint message. Its FIXED text is "<N> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: "; its VARIABLE parts are the error count N, the member name taken from the request (here authenticationMaterial, lowerCamel wire name) and the pattern — and AWS renders the pattern STRIPPED OF ITS ANCHORS, i.e. [0-9A-Za-z!#$%&()*\+\-;<=>?@^_`{|}~\/: {},\\"]+ with no leading ^ and no trailing $. The offending value itself is NOT echoed (the member is @sensitive).
Emulator: same status and code, but the quoted pattern is the raw modelled trait including anchors: ^[0-9A-Za-z!#$%&()*\+\-;<=>?@^_`{|}~\/: {},\\"]+$ — so the message differs from AWS's by exactly the leading '^' and trailing '$'.
Why: the constraint message is part of the error contract. A client that parses the quoted pattern out of the message and recompiles it gets an anchored expression where AWS gave an unanchored one, and any exact-text comparison against AWS fails.
Fix: the shared constraint-violation message builder used by the generated input validation (the service's validation layer that renders @pattern failures), so every operation of this service strips the anchors when rendering a @pattern trait into a ValidationException message — not just CreateManagedThing.

--------------------------------------------------------------------
Cause 5 of 6: CreateNotificationConfiguration's bad-destination refusal uses an emulator-authored message.
AWS: Smithy: CreateNotificationConfiguration, POST /notification-configurations; input DestinationName @required with @length(1..128) and @pattern ^[\p{L}\p{N} ._-]+$; modelled errors include ValidationException (httpError 400, single member Message). Recorded behavior: naming a destination that does not exist is refused HTTP 400 ValidationException with Message "Destination doesn't exist" — the whole string is fixed text, with the submitted destination name NOT interpolated.
Emulator: correct status and code, Message 'The provided DestinationName does not exist.'
Why: message text is a requirement of the error class; a caller matching or displaying AWS's wording diverges.
Fix: CreateNotificationConfiguration's validation error construction (src/ops/create_notification_configuration.rs).
Same root cause as open issue 20261029T171839Z-iot-managed-integrations-error-messages-for-not-found-and-invalid-reference.md (its Cause 5 of 6), filed from the wire corpus with no behavioral replay key.

--------------------------------------------------------------------
Cause 6 of 6: PutRuntimeLogConfiguration's missing-managed-thing refusal words its message wrongly and leaks the submitted id.
AWS: Smithy: PutRuntimeLogConfiguration, PUT /runtime-log-configurations/{ManagedThingId}, success code 200; ManagedThingId is an @required @httpLabel with @length(1..64) and @pattern ^[a-zA-Z0-9:_-]*$; modelled errors include ResourceNotFoundException (httpError 404) and ValidationException (httpError 400). Recorded behavior: for a ManagedThingId no managed thing holds, AWS answers ResourceNotFoundException with Message 'The runtime log configuration is for a managed thing resource that cannot be found.' — the entire sentence is fixed text for this operation, phrased around the runtime-log configuration rather than the thing, and it interpolates NO value from the request.
Emulator: the error class is right (ResourceNotFoundException) but Message is "ManagedThing for identifier '<ManagedThingId>' not found" — a different sentence that additionally interpolates the submitted identifier, which AWS's message does not contain. (The '<ManagedThingId>' here is the request's path label, the variable the emulator wrongly inserts; the particular value in this run is not part of any requirement.)
Why: this operation's not-found text is operation-specific in AWS and generic-per-resource in the emulator, so a client keyed on AWS's wording mis-handles the refusal; the emulator also echoes a caller-supplied identifier AWS keeps out of the body.
Fix: PutRuntimeLogConfiguration's not-found error construction (src/ops/put_runtime_log_configuration.rs) — use this operation's own fixed text instead of the shared '<Resource> for identifier ... not found' helper. The same check asserts further behavior that was never reached in this run and is therefore NOT claimed here: GetRuntimeLogConfiguration's own not-found text ("Runtime log configuration for the requested managedThingId doesn't exist"), and the non-atomic ValidationException 'Only 1P device or hub is supported for Runtime logging' that AWS returns for an existing third-party thing while still persisting the submitted configuration.

Traces: see trace_ids — six behavioral check ids, one per cause, in the order above.

## Evidence
Suite: simparity-behavioral-aws-v1, 10/21 passed; iot-managed-integrations 10/21 (48%), 11 failed. Four failures were marked ALREADY FILED (blocked by unimplemented UpdateCloudConnector, UpdateEventLogConfiguration, DeleteOtaTaskConfiguration, CreateProvisioningProfile) and are excluded. The six below are the surviving failures.

Cause 1 — [mismatch] iot-managed-integrations.account-association-lifecycle-and-refresh-conflict:
  AssertionError: create-account-association-on-deleted-destination: expected a ClientError, call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'OAuthAuthorizationUrl': '', 'AccountAssociationId': '<generated id>', 'AssociationState': 'ASSOCIATION_IN_PROGRESS', 'Arn': 'arn:aws:iotmanagedintegrations:<region>:<account>:account-association/<generated id>'}
  Expected: HTTP 400, Code 'ValidationException', Message 'Connector Destination in the request does not exist.'
  Positive control in the same check PASSED: GetConnectorDestination on that deleted id answered ResourceNotFoundException / 'Connector Destination not found', so the deletion is in state.

Cause 2 — [mismatch] iot-managed-integrations.default-encryption-configuration-lifecycle (line 34):
  assert code_msg == 'IoT Managed Integrations is not authorized to use the key.'
  AssertionError: kmsKeyArn is required when encryptionType is "CUSTOMER_KEY_ENCRYPTION"
  Expected Message: 'IoT Managed Integrations is not authorized to use the key.'  Actual Message: 'kmsKeyArn is required when encryptionType is "CUSTOMER_KEY_ENCRYPTION"'. Code ValidationException matched.

Cause 3 — [mismatch] iot-managed-integrations.destination-lifecycle-and-update (line 48):
  assert code_msg is None
  AssertionError: Destination resource not found.
  Expected: GetDestination after DeleteDestination -> 404 ResourceNotFoundException with NO Message member. Actual: Message 'Destination resource not found.' present. (Code ResourceNotFoundException matched; the post-delete UpdateDestination assertion on an absent Message was not reached.)

Cause 4 — [mismatch] iot-managed-integrations.managed-thing-lifecycle-state-transitions (line 47):
  Expected Message: 1 validation error detected: Value at 'authenticationMaterial' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z!#$%&()*\+\-;<=>?@^_`{|}~\/: {},\\"]+
  Actual   Message: 1 validation error detected: Value at 'authenticationMaterial' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z!#$%&()*\+\-;<=>?@^_`{|}~\/: {},\\"]+$
  Difference is exactly the leading '^' and trailing '$'. Code ValidationException matched. Positive control in the same check PASSED: a well-formed WIFI_SETUP_QR_BAR_CODE material created a managed thing in UNASSOCIATED, so the rejection is attributable to the material pattern.

Cause 5 — [mismatch] iot-managed-integrations.notification-configuration-lifecycle (line 56):
  assert missing_code_msg == "Destination doesn't exist"
  AssertionError: The provided DestinationName does not exist.
  Expected Message: "Destination doesn't exist"  Actual Message: 'The provided DestinationName does not exist.'. Code ValidationException matched.

Cause 6 — [mismatch] iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle (line 45):
  assert missing_message == "The runtime log configuration is for a managed thing resource that cannot be found."
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got "ManagedThing for identifier '<submitted id>' not found"
  Code ResourceNotFoundException matched; only the Message diverges, and it interpolates the submitted ManagedThingId that AWS's message does not contain.

## Steps to Reproduce
Boot the emulator and replay the six behavioral checks in trace_ids (all against the iot-managed-integrations endpoint; the checks pin the client to a supported region).

1. Cause 1 — iot-managed-integrations.account-association-lifecycle-and-refresh-conflict: CreateCloudConnector, CreateConnectorDestination, then DeleteConnectorDestination on it; confirm GetConnectorDestination answers 404. Now POST /account-associations with ConnectorDestinationId set to that deleted id. Emulator answers 201 and creates an association; expect 400 ValidationException 'Connector Destination in the request does not exist.'
2. Cause 2 — iot-managed-integrations.default-encryption-configuration-lifecycle: POST /configuration/account/encryption with encryptionType='CUSTOMER_KEY_ENCRYPTION' and no kmsKeyArn. Compare the 400 ValidationException Message with 'IoT Managed Integrations is not authorized to use the key.'
3. Cause 3 — iot-managed-integrations.destination-lifecycle-and-update: CreateDestination, GetDestination, UpdateDestination, DeleteDestination; then GET /destinations/{name} and PUT /destinations/{name} on the dead name. Assert the 404 ResourceNotFoundException body has NO Message member.
4. Cause 4 — iot-managed-integrations.managed-thing-lifecycle-state-transitions: POST /managed-things with AuthenticationMaterialType='WIFI_SETUP_QR_BAR_CODE' and an AuthenticationMaterial containing characters outside the modelled pattern (e.g. non-ASCII in the serial). Compare the ValidationException Message's quoted pattern — it must be unanchored.
5. Cause 5 — iot-managed-integrations.notification-configuration-lifecycle: POST /notification-configurations with EventType='DEVICE_STATE' and a DestinationName that was never created. Compare the 400 ValidationException Message with "Destination doesn't exist".
6. Cause 6 — iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle: PUT /runtime-log-configurations/{id} for a ManagedThingId no managed thing holds. Compare the 404 ResourceNotFoundException Message with 'The runtime log configuration is for a managed thing resource that cannot be found.' and confirm the submitted id is not interpolated.
