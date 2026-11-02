Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connecthealth.create-subscription-malformed-domain-id-rejected connecthealth.get-patient-insights-job-not-found connecthealth.start-patient-insights-job-conflicting-client-token-rejected connecthealth.start-patient-insights-job-malformed-s3-uri-rejected
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth.create-subscription-malformed-domain-id-rejected|run#12|AssertionError|assert message.startswith(expected), f"expected a message starting {expected!r}, got {message!r}"
Issue-Key: connecthealth.get-patient-insights-job-not-found|_seed_patient#10|Timeout|time.sleep(2)
Issue-Key: connecthealth.start-patient-insights-job-conflicting-client-token-rejected|_seed_patient#11|Timeout|time.sleep(2)
Issue-Key: connecthealth.start-patient-insights-job-malformed-s3-uri-rejected|_seed_patient#10|Timeout|time.sleep(2)
Issue-Commit: e968c00e41b0d1403a091108575069722ed2efa8

# connecthealth: CreateSubscription words a malformed domainId with the generic constraint sentence instead of "<id> is not a valid domain id.", and three patient-insights checks never reach their assertions because the HealthLake FHIR search interaction their setup depends on answers 501 (2 causes)

> _Note: this issue was identified against the code base at commit `e968c00e41b0d1403a091108575069722ed2efa8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
Two distinct root causes in the connecthealth behavioral surface. Both must be closed before submitting.

Cause 1 of 2: CreateSubscription refuses a well-formed-length but wrongly-prefixed domainId with the framework's generic constraint-violation sentence instead of the service's own domain-id refusal.
AWS: Smithy (com-amazonaws-connecthealth-connecthealth.json) models CreateSubscription as POST /domains/{domainId}/subscriptions, input member domainId -> DomainId with @httpLabel, @required, @length(min 20, max 25) and @pattern "^(hai-|dom-)[a-z0-9]+$"; ValidationException is a modelled error with httpError 400. The model states the pattern but does NOT state how the service words a value that violates it. Recorded behavior: a 21-character domainId that satisfies the length bound but carries a foreign prefix is NOT refused by a framework-level constraint walk — it reaches the service, which answers HTTP 400 ValidationException whose Message is the service's own sentence. Its FIXED text is " is not a valid domain id." preceded by the rejected domainId exactly as the request supplied it, followed by the backend's SDK-style nested suffix beginning "(Service: ". So the requirement is: Message starts with "<domainId-from-request> is not a valid domain id. (Service: ". The value before the fixed sentence is the request's domainId; the content inside the "(Service: ...)" suffix is per-request (it carries a Request ID) and only its leading "(Service: " is a requirement — never copy one run's id, status-code or request-id text.
Emulator: the code is right (ValidationException, 400) but the Message is the generic modelled-input-validation sentence: "1 validation error detected: Value 'xxx-00000000000000000' at 'domainId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(hai-|dom-)[a-z0-9]+$". That is the framework constraint walk rendering the modelled @pattern, i.e. the request is refused BEFORE the operation's own handler can word it, so the "is not a valid domain id." sentence is unreachable for every malformed domainId on this path.
Why: a client (or a human reading a CLI error) that matches on, logs or surfaces AWS's domain-id refusal sees a completely different sentence, and tooling that distinguishes "this id is not a valid domain id" from a generic member-constraint violation cannot. The positive control in the same check passes, so only the wording of the refusal diverges — which is exactly the part a caller reads.
Fix: the domainId refusal belongs in CreateSubscription's validation/handler path for this service (the per-operation input validation that the crate's service-level ModeledInputValidation gate currently preempts): the httpLabel domainId must be refused by the operation with the service's own sentence rather than by the generic constraint gate.
Traces: connecthealth.create-subscription-malformed-domain-id-rejected.

Cause 2 of 2: the FHIR search interaction that every connecthealth patient-insights check needs in order to seed a patient is answered 501 not-supported, so three checks burn their whole 60s budget in setup and never reach a single connecthealth assertion.
AWS: StartPatientInsightsJob (POST /domain/{domainId}/patient-insights-job, hostPrefix "runtime.") resolves inputDataConfig.fhirServer.fhirEndpoint against a real HealthLake FHIR datastore and patientContext.patientId against a Patient resource that datastore actually holds, so a caller must first write that Patient over the datastore's FHIR REST data plane and confirm it is indexed. The FHIR interactions on a DatastoreEndpoint are not modelled operations — recorded behavior only: a sigv4-signed (service name "healthlake") search "GET <DatastoreEndpoint>/Patient?identifier=<value>" answers HTTP 200 with a searchset Bundle whose entry[] contains the matching Patient. The checks poll exactly that until it answers 200 with at least one entry.
Emulator: the datastore FHIR REST data plane serves only create, read-by-id and update/update-as-create; every other interaction on that path space — search included — is answered 501 with an OperationOutcome of code "not-supported" (documented in services/service-com-amazonaws-healthlake-healthlake/src/service/fhir_rest.rs, module doc "Every other interaction on this path space (delete, search, vread, the batch/transaction endpoint) is answered 501"). The seeding helper's search poll therefore never succeeds, spins for its 90s budget, and the check is killed at its 60s limit with "Check timed out after 60.0s" having asserted nothing about connecthealth. The one patient-insights check that does NOT poll the search path got through the same setup, which isolates the search interaction as the blocking step.
Why: every connecthealth patient-insights path a client would actually exercise — start a job for a patient that exists, read it back, get a not-found job refused — is unreachable, because the documented precondition (a Patient resolvable in the FHIR datastore) cannot be established. Three behavioral checks are reduced to timeouts, so the emulator's StartPatientInsightsJob/GetPatientInsightsJob fidelity is completely unverified rather than merely wrong. This is NOT covered by the resolved issue 20261022T185152Z-healthlake-non-rpc-http-surfaces-are-unroutable-datastore.md, which fixed routing plus the create and read-by-id interactions and explicitly left each remaining interaction as its own gap; no open issue carries the search interaction.
Fix: implement the FHIR search interaction (GET [base]/{type}?<param>=<value> answering 200 with a searchset Bundle over the resources the datastore holds, at minimum identifier/_id matching) in the HealthLake datastore FHIR REST data plane that connecthealth's patient-insights handlers depend on, so patientContext.patientId can be resolved; file/line citations under Traces.
Traces: connecthealth.get-patient-insights-job-not-found, connecthealth.start-patient-insights-job-conflicting-client-token-rejected, connecthealth.start-patient-insights-job-malformed-s3-uri-rejected; component: services/service-com-amazonaws-healthlake-healthlake/src/service/fhir_rest.rs lines 34-38 (search answered 501 not-supported) reached via services/service-com-amazonaws-healthlake-healthlake/src/service.rs lines 437-441.

## Evidence
Cause 1 — connecthealth.create-subscription-malformed-domain-id-rejected [mismatch]:
  AssertionError: expected a message starting 'xxx-00000000000000000 is not a valid domain id. (Service: ', got "1 validation error detected: Value 'xxx-00000000000000000' at 'domainId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(hai-|dom-)[a-z0-9]+$"
  The error CODE matched (ValidationException) and the positive control (CreateSubscription with a well-formed domainId, then ListSubscriptions == 1) passed, so the divergence is confined to the Message. Fixed text of the requirement: "<domainId> is not a valid domain id. (Service: "; the leading value is the request's domainId (here a 21-char value with a foreign prefix) and the suffix tail after "(Service: " is per-request and not asserted.

Cause 2 — three checks, identical symptom:
  connecthealth.get-patient-insights-job-not-found [mismatch]: Error: Check timed out after 60.0s
  connecthealth.start-patient-insights-job-conflicting-client-token-rejected [mismatch]: Error: Check timed out after 60.0s
  connecthealth.start-patient-insights-job-malformed-s3-uri-rejected [mismatch]: Error: Check timed out after 60.0s
All three share one setup: CreateDomain -> S3 bucket + input.json -> CreateFHIRDatastore (waited out of CREATING to ACTIVE) -> PUT <DatastoreEndpoint>/Patient/<id> (asserted 200/201, so the write itself is accepted) -> poll GET <DatastoreEndpoint>/Patient/<id> until 200 (60s budget) -> poll GET <DatastoreEndpoint>/Patient?identifier=<id> until 200 with entry[] non-empty (90s budget). The search poll cannot be satisfied: the datastore FHIR REST data plane answers every non-create/read/update interaction 501 with OperationOutcome code "not-supported". Each poll budget alone exceeds the check's 60s limit, so the run dies in setup and the connecthealth assertions (ResourceNotFoundException "Job <jobArn> does not exist.", clientToken non-deduplication, ValidationException "Request validation failed" for a malformed s3OutputPath) are never evaluated. The fourth patient-insights check in the suite, which does not poll the search path, completed its setup and reached its connecthealth assertion — isolating the search interaction as the blocking step rather than the datastore control plane or the FHIR write.

## Steps to Reproduce
Cause 1:
1. CreateDomain(name=<unique>) and take domainId.
2. CreateSubscription(domainId=<that domainId>) — succeeds; ListSubscriptions(domainId) returns 1 subscription (positive control).
3. CreateSubscription(domainId="xxx-" + "0"*17) — a 21-character value inside the modelled @length(20,25) bound but with a prefix outside @pattern "^(hai-|dom-)[a-z0-9]+$".
4. Observe HTTP 400 ValidationException whose Message is "1 validation error detected: Value '<value>' at 'domainId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(hai-|dom-)[a-z0-9]+$". Expected: Message starting "<value> is not a valid domain id. (Service: ".

Cause 2:
1. CreateDomain(name=<unique>) and take domainId.
2. S3: CreateBucket(<unique>) and PutObject(Key="input.json").
3. HealthLake: CreateFHIRDatastore(DatastoreTypeVersion="R4", DatastoreName=<unique>, ClientToken=<name>); poll DescribeFHIRDatastore until DatastoreStatus == ACTIVE; take DatastoreProperties.DatastoreEndpoint.
4. SigV4-sign (service "healthlake") PUT {DatastoreEndpoint}/Patient/{patientId} with Content-Type application/fhir+json and a minimal Patient carrying identifier[0].value == patientId. It answers 200/201.
5. SigV4-sign GET {DatastoreEndpoint}/Patient?identifier={patientId}. Observe HTTP 501 with an OperationOutcome whose issue code is "not-supported". Expected: HTTP 200 with a searchset Bundle whose entry[] holds the Patient written in step 4.
6. Because the seeding helper loops on step 5 for up to 90s, the enclosing connecthealth check is killed at its 60s limit before StartPatientInsightsJob / GetPatientInsightsJob is ever called.
