Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-contacts-create-contact-without-replication-set-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-contacts-create-contact-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts|CreateContact|4xx->2xx|$.Fields
Issue-Key: ssm-contacts|CreateContact|4xx->2xx|$.Message
Issue-Key: ssm-contacts|CreateContact|4xx->2xx|$.Reason
Issue-Key: ssm-contacts|CreateContact|4xx->2xx|$.__type
Issue-Key: ssm-contacts|CreateContact|4xx->2xx|status
Issue-Commit: 07653fe6d7ae123674bcbfa9037d42ab6af0ac40

# CreateContact succeeds with no replication set instead of ValidationException

> _Note: this issue was identified against the code base at commit `07653fe6d7ae123674bcbfa9037d42ab6af0ac40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateContact

## Description
SSM Contacts CreateContact is accepted and a contact is created even when the account has no replication set configured, where AWS refuses the request.

Cause 1 of 1: CreateContact against an account with no replication set must be rejected, not serviced (fixture ssm-contacts-create-contact-without-replication-set-rejected, step task#1).

AWS: Amazon SSM Contacts is a global service fronted by a per-account replication set; until one exists the account has no home region for contact resources and contact-creating calls are refused rather than serviced. CreateContact models ValidationException as a client error with HTTP 400 (Smithy: CreateContact errors include ValidationException, errorKind client, httpError 400; ValidationException members Message (required, String), Reason (ValidationExceptionReason enum), Fields (ValidationExceptionFieldList)). The recorded response for this precondition is HTTP 400 with __type 'ValidationException', a populated Message, a Reason, and Fields present as an empty list. The specific wording is recorded behavior only: the recording shows Message 'Invalid value provided - Account not found for the request' and Reason 'Account not found for the request' — note the recorded Reason text is not one of the modeled ValidationExceptionReason enum values (UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER), so treat those two strings as examples and the status plus error code as the requirement. Hypothesis: the real service resolves the caller's replication set before admitting the write, and the absent replication set surfaces as a validation failure rather than a ResourceNotFoundException.

Emulator: the step returns HTTP 200 with a CreateContact success body. The replay reports 'status: expected 400, got 200' and all four error members absent: $.__type missing (expected 'ValidationException'), $.Fields missing (expected []), $.Message missing, $.Reason missing. The follow-on step in the same fixture (task#2) diverges identically, so the emulator keeps serving the account as though a replication set were present.

Why: a client that provisions SSM Contacts resources relies on this 400 to detect that the account has not been onboarded (no replication set) and to run its replication-set bootstrap first. Against the emulator the call appears to succeed, so the client skips onboarding and proceeds with a ContactArn for a contact that could not exist in AWS; error-path tests that assert ValidationException/400 pass vacuously, and any retry-or-create logic keyed on that error never executes.

Fix: the CreateContact handler's precondition check in the ssm-contacts service — it must consult the account's replication-set state (service state) before admitting the create, and on absence return the modeled ValidationException through the error serializer with HTTP 400, __type 'ValidationException', a Message, a Reason, and the Fields list present (empty when no specific input field is at fault). Today no such precondition appears to be consulted at all.

Traces: ssm-contacts-create-contact-without-replication-set-rejected#task#1 (representative) and #task#2 in the same cluster (affected_count 2).

## Evidence
Fixture ssm-contacts-create-contact-without-replication-set-rejected, step task#1, operation CreateContact (service ssm-contacts), verdict mismatch.

Expected (recorded AWS): HTTP 400, body with $.__type = 'ValidationException', $.Message = 'Invalid value provided - Account not found for the request' (example text), $.Reason = 'Account not found for the request' (example text), $.Fields = [].

Actual (emulator): HTTP 200 with a success body; $.__type, $.Message, $.Reason and $.Fields all absent.

Replay detail verbatim: "status: expected 400, got 200 / $.__type: missing (expected 'ValidationException', got None) / $.Fields: missing (expected [], got None) / $.Message: missing (expected 'Invalid value provided - Account not found for the request', got None) / $.Reason: missing (expected 'Account not found for the request', got None)".

Cluster: divergence_key service=ssm-contacts|CreateContact|status=400->200|keys=$.Fields,$.Message,$.Reason,$.__type,status; affected = ssm-contacts-create-contact-without-replication-set-rejected#task#1, #task#2 (affected_count 2). No stub_path and no owned_by, so this is net-new. Model reference: Smithy CreateContact -> ValidationException (client, httpError 400); ValidationException { Message required, Reason: ValidationExceptionReason enum, Fields: ValidationExceptionFieldList }.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) and look at fixture ssm-contacts-create-contact-without-replication-set-rejected.
3. Step task#1 issues CreateContact (SSMContacts.CreateContact, JSON RPC POST) for an account that has no replication set configured, with the fixture's Alias, Type and Plan.
4. Observe the emulator answers HTTP 200 with a CreateContact success body containing a ContactArn.
5. Expected: HTTP 400 with __type 'ValidationException' and the Message/Reason/Fields members populated (Fields present, empty when no input field is at fault).
6. Step task#2 of the same fixture repeats the condition and diverges the same way; re-replay both steps to confirm a fix.
