Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appintegrations.create-application-invalid-namespace-pattern-rejected appintegrations.create-event-integration-get-roundtrip appintegrations.delete-event-integration-removes-from-list appintegrations.delete-data-integration-nonexistent-rejected-and-real-delete-removes-resource appintegrations.create-data-integration-nonexistent-kms-key-rejected
Issue-Shape-Id: com.amazonaws.appintegrations#AmazonAppIntegrationService
Issue-Key: appintegrations.create-application-invalid-namespace-pattern-rejected|run#6|AssertionError|assert code == "BadRequestException", code
Issue-Key: appintegrations|GetEventIntegration|ResourceNotFoundException
Issue-Key: appintegrations.delete-event-integration-removes-from-list|run#15|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: appintegrations.delete-data-integration-nonexistent-rejected-and-real-delete-removes-resource|run#6|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: appintegrations.create-data-integration-nonexistent-kms-key-rejected|run#9|AssertionError|assert code == "InvalidRequestException", code
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# appintegrations: constraint rejections answer ValidationException instead of BadRequestException, ResourceNotFoundException messages use a non-AWS template, DeleteDataIntegration is a 501 stub, and CreateDataIntegration never validates KmsKey

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication, GetEventIntegration, DeleteDataIntegration, CreateDataIntegration

## Description
Four distinct root causes in the AppIntegrations emulator, all surfaced by the behavioral suite (2/19 checks pass). The service is bootstrapped and answering, so these are fidelity defects, not bootstrap gaps.

Cause 1 of 4: a modeled-constraint violation on CreateApplication is refused with the wrong error code.
AWS: Smithy model CreateApplication.Namespace -> ApplicationNamespace, @required, @length min 1 max 211, @pattern ^[a-zA-Z0-9\/\._\-]+$ (model: services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json). A Namespace that violates that pattern (e.g. one containing a space and '!') is refused by the AppIntegrations REST front end before dispatch; the recorded response is HTTP 400 with error code BadRequestException and message "Invalid request body" (recorded behavior only — BadRequestException is not one of CreateApplication's modeled errors, so the status/code come from the recording, and the message is the service's fixed body-validation text).
Emulator: the centralized constraint-validation step refuses the same request with error code ValidationException instead of BadRequestException.
Why: a client that branches on the error code (retry/repair on BadRequestException, surface-as-bug on an unexpected code) takes the wrong branch, and SDK error-type matching for this service never matches.
Fix: the shared request-validation/error-serialization layer for this service crate (the code that turns a modeled length/pattern/required violation into a wire error for appintegrations) must emit BadRequestException for body-constraint violations on this service.
Traces: appintegrations.create-application-invalid-namespace-pattern-rejected.

Cause 2 of 4: ResourceNotFoundException messages do not follow AWS's templated text.
AWS: GetEventIntegration models ResourceNotFoundException (HTTP 404). AWS builds that message from the request identifier with the family template "Could not find <ResourceType> with <name|identifier>: <value>." — for event integrations "Could not find EventIntegration with name: <Name>.", and the same recorded shape appears for the sibling resources ("Could not find DataIntegration with identifier: <id>.", "Could not find Application with identifier: <arn>."). The rule is request-derived and consistent across resources, so the template is the requirement (the concrete name in any one run is only an example).
Emulator: services/.../src/ops/get_event_integration.rs:163 formats "No event integration named {name} exists.". Status 404 and code ResourceNotFoundException are right; the message template is not. The same wrong template is reached from the post-delete read.
Why: clients and operators that parse or assert on the not-found text (and the suite's own cross-resource assertions) see a message that cannot be produced by AWS, so not-found handling that keys on the message fails.
Fix: the not-found construction in the EventIntegration handlers (get_event_integration.rs, and every other appintegrations handler that constructs ResourceNotFoundException) should use the AWS template, interpolating the requested identifier.
Traces: appintegrations.create-event-integration-get-roundtrip, appintegrations.delete-event-integration-removes-from-list.

Cause 3 of 4: DeleteDataIntegration is an unimplemented scaffold stub, so it answers 501 NotImplemented for every request, including the not-found case.
AWS: Smithy: DeleteDataIntegration, DELETE /dataIntegrations/{DataIntegrationIdentifier}, success 200 with an empty body; modeled errors include ResourceNotFoundException (HTTP 404) and InvalidRequestException (HTTP 400). Deleting a nonexistent identifier yields ResourceNotFoundException, and the recorded message follows the Cause 2 template ("Could not find DataIntegration with identifier: <id>."); deleting an existing DataIntegration succeeds and the resource is then gone for reads and mutations alike.
Emulator: services/service-com-amazonaws-appintegrations-amazonappintegrationservice/src/ops/delete_data_integration.rs:161 returns Scaffolded::not_implemented("DeleteDataIntegration") (HTTP 501, code NotImplemented) unconditionally; the file still carries the SCAFFOLDED marker at line 4. No issue in the repo covers this operation (the suite's pre-filed stub issues name CreateApplication and CreateDataIntegration only).
Why: DataIntegrations can never be deleted, so no caller can clean up or observe delete semantics, and the 501/NotImplemented pair is not a code any AWS client is prepared to classify.
Fix: implement handle() in delete_data_integration.rs against the service state (remove the record, 404 with the templated message when absent, refuse delete of a DataIntegration that still has associations per the operation's documented rule) and drop the SCAFFOLDED marker line.
Traces: appintegrations.delete-data-integration-nonexistent-rejected-and-real-delete-removes-resource.

Cause 4 of 4: CreateDataIntegration performs no KmsKey existence/accessibility validation — today it cannot, because the operation is still a stub; this cause records the validation requirement that the separately filed "CreateDataIntegration unimplemented scaffold stub" issue does not state.
AWS: Smithy: CreateDataIntegration.KmsKey -> NonBlankString, @required, @length 1..255, pattern \S; modeled errors include InvalidRequestException (HTTP 400). A well-formed but nonexistent KMS key ARN (arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000) is rejected with InvalidRequestException and no DataIntegration is persisted (it does not appear in ListDataIntegrations); the recorded message embeds the KMS request id of the service's own failed key lookup, so only the status and code are requirements. A real, accessible key makes the identical call succeed, and GetDataIntegration echoes KmsKey back.
Emulator: the call answers HTTP 501 error code NotImplemented, so the KMS key is neither resolved nor rejected and there is no accept/reject distinction at all.
Why: a client gets NotImplemented where AWS gives a classifiable 400, and once the stub is implemented without this check an unusable DataIntegration (encrypted with a key that does not exist) would be accepted silently.
Fix: when implementing create_data_integration.rs, resolve KmsKey against the KMS store and refuse an unresolvable/inaccessible key with InvalidRequestException, persisting nothing; echo the accepted KmsKey from Get/Create.
Traces: appintegrations.create-data-integration-nonexistent-kms-key-rejected.

## Evidence
Suite: appintegrations 2/19 passed (11%), 17 failed.

Cause 1 — appintegrations.create-application-invalid-namespace-pattern-rejected [mismatch]:
  expected error code BadRequestException (HTTP 400, message "Invalid request body") for Namespace="invalid namespace!"
  got    AssertionError: ValidationException

Cause 2 — appintegrations.create-event-integration-get-roundtrip [mismatch]:
  AssertionError: expected 'Could not find EventIntegration with name: sim2real-evt-2536d8256127.', got 'No event integration named sim2real-evt-2536d8256127 exists.'
  appintegrations.delete-event-integration-removes-from-list [mismatch]:
  AssertionError: expected 'Could not find EventIntegration with name: sim2real-evt-del-e7f67a5e8a31.', got 'No event integration named sim2real-evt-del-e7f67a5e8a31 exists.'
  (code ResourceNotFoundException and 404 both matched; only the message template diverges)
  Source: src/ops/get_event_integration.rs:161-165 formats "No event integration named {} exists."

Cause 3 — appintegrations.delete-data-integration-nonexistent-rejected-and-real-delete-removes-resource [mismatch]:
  delete_data_integration(DataIntegrationIdentifier="00000000-0000-0000-0000-000000000000")
  expected ResourceNotFoundException (404) with 'Could not find DataIntegration with identifier: 00000000-0000-0000-0000-000000000000.'
  got    AssertionError: NotImplemented (HTTP 501, "operation DeleteDataIntegration is scaffolded but not implemented in this emulator")
  Source: src/ops/delete_data_integration.rs:4 (SCAFFOLDED marker), :161 (not_implemented).

Cause 4 — appintegrations.create-data-integration-nonexistent-kms-key-rejected [mismatch]:
  create_data_integration(Name=..., KmsKey="arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000", SourceURI="s3://<bucket>")
  expected InvalidRequestException (400), nothing persisted
  got    AssertionError: NotImplemented (HTTP 501)

## Steps to Reproduce
Boot the emulator and point a botocore appintegrations client at it.

Cause 1: create_application(Name="ok-name", Namespace="invalid namespace!", ApplicationSourceConfig={"ExternalUrlConfig":{"AccessUrl":"https://example.com/bad"}}) -> expect 400 BadRequestException; observe ValidationException.

Cause 2: create_event_integration(Name=N, EventFilter={"Source":"aws.cases"}, EventBridgeBus="default"); delete_event_integration(Name=N); get_event_integration(Name=N) -> expect 404 ResourceNotFoundException with message "Could not find EventIntegration with name: <N>."; observe "No event integration named <N> exists.".

Cause 3: delete_data_integration(DataIntegrationIdentifier="00000000-0000-0000-0000-000000000000") -> expect 404 ResourceNotFoundException; observe 501 NotImplemented.

Cause 4: create_data_integration(Name=<unique>, KmsKey="arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000", SourceURI="s3://<existing-bucket>") -> expect 400 InvalidRequestException and the name absent from list_data_integrations; observe 501 NotImplemented.

Or replay: run_behavioral_suite and inspect the five checks named in trace_ids.
