Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.create-replication-set-deprecated-rejected ssm-contacts.get-replication-set-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.create-replication-set-deprecated-rejected|run#3|AssertionError|assert code == "UnsupportedOperationException", f"expected UnsupportedOperationException, got {code}"
Issue-Key: ssm-contacts.get-replication-set-nonexistent-arn-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Incident Manager replication-set lifecycle unimplemented: CreateReplicationSet and GetReplicationSet answer 501 NotImplemented instead of the deprecation refusal and 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationSet, GetReplicationSet

## Description
Two ssm-incidents replication-set operations that the ssm-contacts behavioral checks depend on are scaffold stubs and answer HTTP 501 with code NotImplemented. They are the operations that define whether an account is onboarded to Incident Manager at all, so their absence also makes the ssm-contacts precondition contract untestable. 2 causes. (Service token taken from the failing check ids, which are ssm-contacts.*; the handlers live in the ssm-incidents crate.)

Cause 1 of 2: CreateReplicationSet must refuse as deprecated, not answer 501.
AWS: recorded behavior only — the Smithy model for CreateReplicationSet declares errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400) and success HTTP 201 POST /createReplicationSet; it does NOT declare the code the service now returns. The recording pins the current behavior: AWS stopped accepting new Incident Manager customers on 2025-11-07, and CreateReplicationSet now unconditionally fails with error code `UnsupportedOperationException`, recorded message "CreateReplicationSet API is deprecated since Nov 7, 2025." No replication set is ever created.
Emulator: HTTP 501, code NotImplemented (services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/create_replication_set.rs is a scaffold stub). An emulator that instead let CreateReplicationSet SUCCEED would be worse — overly permissive relative to the current service — so the fix is to return the refusal, not to implement creation.
Why: a client's onboarding path must learn that Incident Manager can no longer be enabled; a 501 reads as an emulator gap and sends the client into retry loops instead of the permanent-failure branch.
Fix: implement the handler in services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/create_replication_set.rs to return the deprecation refusal with code UnsupportedOperationException (and delete the SCAFFOLDED marker); since the code is not in the operation's modeled error closure, it must be emitted through the service's error serializer as an unmodeled/common error shape.
Traces: ssm-contacts.create-replication-set-deprecated-rejected.

Cause 2 of 2: GetReplicationSet on an unknown ARN must be 404 ResourceNotFoundException, not 501.
AWS: Smithy GetReplicationSet models AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(client, httpError 404), ThrottlingException(429), ValidationException(400); input `arn` (httpQuery "arn", required, len<=1000, pattern ^arn:aws(-cn|-us-gov)?:...), GET /getReplicationSet, readonly. For a syntactically well-formed but never-created replication-set ARN the recorded response is HTTP 404 / code ResourceNotFoundException with a message built from the request, example "Replication set '<arn>' is not found". This is deliberately a DIFFERENT class from the 400 ValidationException the ssm-contacts data plane returns for a missing account, and the checks assert that distinction.
Emulator: HTTP 501, code NotImplemented.
Why: a client that probes GetReplicationSet to decide whether the account is onboarded cannot distinguish "no replication set" (404) from "not implemented", so onboarding detection is impossible against the emulator.
Fix: implement services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/get_replication_set.rs — return the modeled ResourceNotFoundException (404) for an ARN with no stored replication set.
Traces: ssm-contacts.get-replication-set-nonexistent-arn-rejected.

Related and filed separately: ssm-incidents ListReplicationSets is an unimplemented stub with its own issue (it blocks ssm-contacts.create-rotation-override-without-replication-set-rejected); it is NOT part of this issue.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service ssm-contacts. Verbatim failures:

- ssm-contacts.create-replication-set-deprecated-rejected — `assert code == "UnsupportedOperationException"` -> AssertionError: expected UnsupportedOperationException, got NotImplemented. Check also asserts message "CreateReplicationSet API is deprecated since Nov 7, 2025."
- ssm-contacts.get-replication-set-nonexistent-arn-rejected — `assert code == "ResourceNotFoundException"` -> AssertionError: expected ResourceNotFoundException, got NotImplemented. Check also asserts message f"Replication set '{fake_arn}' is not found" (built from the request ARN).

Expected: CreateReplicationSet -> client error with code UnsupportedOperationException and no replication set created (recorded behavior; not in the operation's modeled error list, which is AccessDenied/Conflict/InternalServer/ServiceQuotaExceeded/Throttling/Validation per Smithy). GetReplicationSet -> HTTP 404 with Error.Code "ResourceNotFoundException" (modeled: errorKind client, httpError 404).

Actual (emulator): both return HTTP 501 with Error.Code "NotImplemented" and message "operation <Op> is scaffolded but not implemented in this emulator".

The ssm-contacts positive control in these checks (ListContacts) answers normally, so the failure is per-operation, not a service outage.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Point a botocore ssm-incidents client at the emulator.
3. `create_replication_set(regions={<region>: {}})` -> emulator raises ClientError 501 / NotImplemented. Expected: a client-side refusal with Error.Code "UnsupportedOperationException" and message "CreateReplicationSet API is deprecated since Nov 7, 2025.", with no replication set created.
4. `get_replication_set(arn="arn:aws:ssm-incidents::<account>:replication-set/<never-created-uuid>")` -> emulator raises ClientError 501 / NotImplemented. Expected: HTTP 404 with Error.Code "ResourceNotFoundException" and a message naming the supplied ARN, e.g. "Replication set '<arn>' is not found".
5. Re-replay ssm-contacts.create-replication-set-deprecated-rejected and ssm-contacts.get-replication-set-nonexistent-arn-rejected to confirm a fix.
