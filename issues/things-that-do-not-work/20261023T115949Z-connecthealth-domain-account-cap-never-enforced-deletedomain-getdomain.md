Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connecthealth.create-domain-quota-exceeded connecthealth.delete-domain-not-found connecthealth.get-domain-not-found connecthealth.delete-domain-then-get-returns-not-found connecthealth.get-medical-scribe-listening-session-not-found connecthealth.get-medical-scribe-listening-session-cross-subscription-rejected connecthealth.get-patient-insights-job-not-found connecthealth.start-patient-insights-job-malformed-s3-uri-rejected connecthealth.start-patient-insights-job-nonexistent-domain-not-found
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth.create-domain-quota-exceeded|run#8|AssertionError|assert quota_code == "ValidationException", (
Issue-Key: connecthealth.delete-domain-not-found|run#4|AssertionError|assert code == "AuthorizerConfigurationException", f"expected AuthorizerConfigurationException, got {code!r}"
Issue-Key: connecthealth.get-domain-not-found|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: connecthealth.delete-domain-then-get-returns-not-found|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: connecthealth.get-medical-scribe-listening-session-not-found|run#10|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: connecthealth.get-medical-scribe-listening-session-cross-subscription-rejected|run#11|AssertionError|assert code_home == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code_home!r}"
Issue-Key: connecthealth.get-patient-insights-job-not-found|_seed_patient#10|AssertionError|assert status in (200, 201), f"failed to seed Patient/{patient_id}: {status} {resp}"
Issue-Key: connecthealth.start-patient-insights-job-malformed-s3-uri-rejected|_seed_patient#10|AssertionError|assert status in (200, 201), f"failed to seed Patient/{patient_id}: {status} {resp}"
Issue-Key: connecthealth.start-patient-insights-job-nonexistent-domain-not-found|_put_fhir_patient#11|AssertionError|assert status in (200, 201), f"expected the FHIR Patient PUT to succeed, got {status}: {resp}"
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# connecthealth: Domain account cap never enforced, DeleteDomain/GetDomain not-found semantics wrong, GetMedicalScribeListeningSession answers NotImplemented, and the FHIR datastore endpoint rejects Patient PUT with 501

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain, DeleteDomain, GetDomain, GetMedicalScribeListeningSession, StartPatientInsightsJob

## Description
Bundled report of 5 distinct root causes found in the connecthealth emulator by the behavioral suite (connecthealth: 10/21 passed). Each cause below is independent and must be closed separately; the two failures the suite marked ALREADY FILED (TagResource / ListTagsForResource scaffold stubs) are deliberately excluded.

=== Cause 1 of 5: CreateDomain never enforces the account-wide Domain cap ===
AWS: CreateDomain is capped per account. Smithy (com-amazonaws-connecthealth-connecthealth): CreateDomain declares exactly one error, ServiceQuotaExceededException (httpError 402); the recorded behavior shows the cap is actually surfaced as a ValidationException (HTTP 400) whose message is built from the caller's account and the cap, of the form "Account <accountId> has reached the maximum limit of <n> domains." (default n = 10). A slot freed by DeleteDomain becomes reusable, so the cap is a live count of existing Domains, not a monotonic counter.
Emulator: CreateDomain accepts an unbounded number of Domains. The check issued 30 consecutive CreateDomain calls and every one succeeded; no error was ever raised (got code=None after 30 successes).
Why: a client that handles the documented quota path — backing off, deleting a Domain to free a slot, or surfacing the cap to a user — is never exercised, and capacity-planning or cap-handling code built against the emulator silently diverges the first time it meets real AWS. Tests that assert the cap cannot be written at all.
Fix: CreateDomain's handler plus the Domain state store: track the live Domain count per account and reject once the cap is reached with HTTP 400 / error code ValidationException and a message naming the account and the cap; DeleteDomain must decrement the live count so the slot is reusable.
Traces: connecthealth.create-domain-quota-exceeded

=== Cause 2 of 5: DeleteDomain on a never-allocated domainId returns the wrong error class ===
AWS: recorded behavior only. DeleteDomain on a well-formed domainId (matching ^(hai-|dom-)[a-z0-9]+$, 20-25 chars) that was never allocated to any account answers error code AuthorizerConfigurationException with a null/absent Message, NOT the ResourceNotFoundException the Smithy model declares as DeleteDomain's only error (httpError 404). Hypothesis (as recorded in the check): the front door cannot resolve an ARN for an id with no allocation history, so the authorizer's own failure surfaces before resource lookup. The requirement here is the error code and the absent message, not any particular message text.
Emulator: returns error code ResourceNotFoundException for the never-allocated id.
Why: a client distinguishing "this id was never valid anywhere" (an authorizer/config failure, generally not retryable and not a 404 to be swallowed) from "this Domain is gone" takes the wrong branch — deletes that should surface a configuration fault are treated as benign idempotent no-ops.
Fix: DeleteDomain's handler / its id-resolution path: separate an id that has no allocation history at all from an id this account once owned, and return AuthorizerConfigurationException with no Message for the former while keeping ResourceNotFoundException for the latter.
Traces: connecthealth.delete-domain-not-found

=== Cause 3 of 5: ResourceNotFoundException message for Domain lookups does not follow AWS's format ===
AWS: recorded behavior only, but the rule is known because the message is built from the request: Domain-not-found responses carry a Message of the form "Domain not found: <domainId>" (the requested domainId, verbatim). Error code ResourceNotFoundException, HTTP 404 (Smithy: ResourceNotFoundException httpError 404 on GetDomain and DeleteDomain). This holds both for GetDomain on an id that never existed and for GetDomain/DeleteDomain after the Domain was deleted (including the second, double-delete call).
Emulator: the code and status are right, but the message is assembled with the opposite template: "Domain <domainId> was not found" (observed e.g. 'Domain dom-zzzzzzzzzzzzzzzzzzzzz was not found' and 'Domain dom-4bce998180824a57 was not found').
Why: clients and operators that parse or match on the not-found message — log scrapers, assertion suites, support tooling keying off the documented wording — do not match, and the divergence is in a message whose generating rule is fixed and request-derived, so it is a wire requirement rather than an incidental string.
Fix: the shared Domain not-found error constructor used by GetDomain's and DeleteDomain's handlers (the error-rendering helper, not individual call sites): format as "Domain not found: {domainId}".
Traces: connecthealth.get-domain-not-found, connecthealth.delete-domain-then-get-returns-not-found

=== Cause 4 of 5: GetMedicalScribeListeningSession answers NotImplemented instead of its modeled errors ===
AWS: Smithy: GetMedicalScribeListeningSession GET /medical-scribe-stream/domain/{domainId}/subscription/{subscriptionId}/session/{sessionId} (hostPrefix streaming.), errors AccessDeniedException(401), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429). A well-formed sessionId (36-char UUID) that was never started, scoped to a real existing domainId/subscriptionId, is rejected with ResourceNotFoundException (HTTP 404); a sessionId looked up under a subscription other than the one it belongs to is likewise rejected with ResourceNotFoundException rather than being leaked across subscriptions. Recorded message example: "Resource not found: sessionId with ID <sessionId> does not exist".
Emulator: both the not-found path and the cross-subscription path answer error code NotImplemented, so the operation has no working lookup or scoping at all.
Why: NotImplemented is not in the operation's modeled error set, so an SDK client gets an unmodeled error it cannot deserialize into a typed exception or handle; no caller can implement session polling, and — worse for tenancy — there is no evidence the emulator scopes a session to its subscription, so cross-subscription isolation cannot be relied on.
Fix: the GetMedicalScribeListeningSession handler and the scribe-session state lookup: resolve the session keyed by (domainId, subscriptionId, sessionId) and return ResourceNotFoundException (HTTP 404) when no such session exists under that exact scope.
Traces: connecthealth.get-medical-scribe-listening-session-not-found, connecthealth.get-medical-scribe-listening-session-cross-subscription-rejected

=== Cause 5 of 5: the FHIR datastore data-plane endpoint rejects Patient PUT with HTTP 501, blocking every patient-insights check ===
AWS: recorded behavior only (FHIR REST data plane of the HealthLake datastore the connecthealth Domain is wired to, reached at the datastore's FHIR endpoint). Seeding a patient by id uses the FHIR update-as-create interaction, PUT [base]/Patient/{id}, which succeeds with HTTP 200 (existing resource) or 201 (created), returning the stored resource; the resource then becomes readable via GET [base]/Patient/{id} and searchable.
Emulator: PUT [base]/Patient/{id} answers HTTP 501 with an OperationOutcome of severity fatal, code not-supported, diagnostics "This data store endpoint implements the FHIR create (POST [base]/{type}) and read (GET [base]/{type}/{id}) interactions; PUT [base]/Patient/<id> is not implemented yet". Only POST-create and GET-read are routed.
Why: no caller can place a resource at a caller-chosen id, which is the normal way clinical data is seeded and re-synced idempotently; downstream connecthealth behavior that depends on a known Patient id is unreachable. Concretely this blocked three otherwise-independent assertions from ever reaching the operation under test: GetPatientInsightsJob's not-found path, StartPatientInsightsJob's malformed-s3OutputPath rejection (expected ValidationException, HTTP 400, message "Request validation failed", and no job persisted under the rejected clientToken so an identical retry re-fails validation), and StartPatientInsightsJob against a well-formed but nonexistent domainId (expected AccessDeniedException, HTTP 401 per the model, message example "Domain is not valid."). Those three expectations remain unverified until the PUT route exists.
Fix: the FHIR data-plane router/handler of the HealthLake datastore endpoint (services/service-com-amazonaws-healthlake-healthlake/src, the FHIR REST surface the datastore endpoint serves): implement the update/update-as-create interaction PUT [base]/{type}/{id} with 200 on replace and 201 on create, storing under the caller-supplied id so the subsequent read and search succeed. NOTE: the failures are reported under the connecthealth check ids because that is the suite's service attribution; the defective component is the HealthLake FHIR endpoint, which is itself bootstrapped.
Traces: connecthealth.get-patient-insights-job-not-found, connecthealth.start-patient-insights-job-malformed-s3-uri-rejected, connecthealth.start-patient-insights-job-nonexistent-domain-not-found

## Evidence
Suite: simparity-behavioral-aws-v1, connecthealth 10/21 passed (48%), 11 failed (9 filed here, 2 ALREADY FILED and skipped).

Cause 1 - connecthealth.create-domain-quota-exceeded [mismatch]
  Expected: ClientError code ValidationException (HTTP 400), Message "Account <accountId> has reached the maximum limit of 10 domains."; then DeleteDomain frees a slot and a retry CreateDomain returns status ACTIVE.
  Actual:   30/30 CreateDomain calls succeeded. AssertionError: "expected the account-cap ValidationException within 30 creates, got code=None after 30 successes".

Cause 2 - connecthealth.delete-domain-not-found [mismatch]
  Request: DeleteDomain(domainId="dom-22222222222222222222") - well-formed, never allocated.
  Expected: code AuthorizerConfigurationException, Message null.
  Actual:   code 'ResourceNotFoundException'. AssertionError: "expected AuthorizerConfigurationException, got 'ResourceNotFoundException'".

Cause 3 - connecthealth.get-domain-not-found [mismatch]
  Expected Message: 'Domain not found: dom-zzzzzzzzzzzzzzzzzzzzz'
  Actual   Message: 'Domain dom-zzzzzzzzzzzzzzzzzzzzz was not found'   (code ResourceNotFoundException, as expected)
  connecthealth.delete-domain-then-get-returns-not-found [mismatch]
  Expected Message: 'Domain not found: dom-4bce998180824a57'
  Actual   Message: 'Domain dom-4bce998180824a57 was not found'

Cause 4 - connecthealth.get-medical-scribe-listening-session-not-found [mismatch]
  Expected: code ResourceNotFoundException (HTTP 404) for a never-started 36-char sessionId under a real domain/subscription.
  Actual:   code 'NotImplemented'. AssertionError: "expected GetMedicalScribeListeningSession on a never-started sessionId to raise ResourceNotFoundException, got 'NotImplemented'".
  connecthealth.get-medical-scribe-listening-session-cross-subscription-rejected [mismatch]
  Expected: code ResourceNotFoundException for both the home-subscription and the cross-subscription lookup.
  Actual:   code 'NotImplemented'. AssertionError: "expected ResourceNotFoundException, got 'NotImplemented'".

Cause 5 - connecthealth.get-patient-insights-job-not-found / .start-patient-insights-job-malformed-s3-uri-rejected / .start-patient-insights-job-nonexistent-domain-not-found [all mismatch]
  Request: PUT <fhir-endpoint>/Patient/<caller-chosen id>
  Expected: HTTP 200 or 201 with the stored resource.
  Actual:   HTTP 501 {'resourceType': 'OperationOutcome', 'issue': [{'severity': 'fatal', 'code': 'not-supported', 'diagnostics': 'This data store endpoint implements the FHIR create (POST [base]/{type}) and read (GET [base]/{type}/{id}) interactions; PUT [base]/Patient/<id> is not implemented yet'}]}
  All three checks aborted in their seeding helper (_seed_patient / _put_fhir_patient) before reaching their connecthealth assertion.

## Steps to Reproduce
Boot the emulator and point a botocore client at the connecthealth endpoint (and, for Cause 5, at the HealthLake FHIR datastore endpoint). Replay the checks listed in trace_ids, or by hand:

Cause 1: call CreateDomain(name=<unique>) in a loop up to 30 times with a fresh account. Observe every call succeeds; AWS rejects once 10 live Domains exist with ValidationException "Account <accountId> has reached the maximum limit of 10 domains.". Then DeleteDomain one Domain, wait for GetDomain to report ResourceNotFoundException, and CreateDomain again - the retry must succeed with status ACTIVE.

Cause 2: call DeleteDomain(domainId="dom-" + "2"*20) (well-formed, never allocated). Observe ResourceNotFoundException; expect AuthorizerConfigurationException with a null Message.

Cause 3: call GetDomain(domainId="dom-" + "z"*21). Inspect Error.Message: observe "Domain <id> was not found"; expect "Domain not found: <id>". Repeat after CreateDomain + DeleteDomain of a real Domain (GetDomain and the second DeleteDomain) for the same message.

Cause 4: create a Domain and a subscription, then call GetMedicalScribeListeningSession(domainId=<real>, subscriptionId=<real>, sessionId=<fresh uuid4>). Observe code NotImplemented; expect ResourceNotFoundException (HTTP 404). Repeat with a sessionId belonging to a different subscriptionId - it must also be ResourceNotFoundException, never a cross-subscription hit.

Cause 5: create a HealthLake FHIR datastore, wait for ACTIVE, then PUT <fhir-endpoint>/Patient/sim2real-patient-1 with a minimal Patient body. Observe HTTP 501 not-supported; expect 200/201 followed by a successful GET <fhir-endpoint>/Patient/sim2real-patient-1. With that route working, re-run the three patient-insights checks to reach their actual assertions (GetPatientInsightsJob ResourceNotFoundException; StartPatientInsightsJob ValidationException "Request validation failed" for outputDataConfig.s3OutputPath="not-an-s3-uri" with no job persisted under the clientToken; StartPatientInsightsJob AccessDeniedException "Domain is not valid." for a nonexistent domainId).
