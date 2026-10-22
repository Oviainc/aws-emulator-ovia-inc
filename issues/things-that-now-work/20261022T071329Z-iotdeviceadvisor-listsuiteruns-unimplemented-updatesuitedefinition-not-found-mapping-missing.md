Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotdeviceadvisor.list-suite-runs-malformed-version-filter-rejected iotdeviceadvisor.update-suite-definition-nonexistent-id-rejected iotdeviceadvisor.create-get-delete-suite-definition-lifecycle iotdeviceadvisor.get-suite-definition-nonexistent-id-not-found iotdeviceadvisor.create-suite-definition-missing-root-group-rejected
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor.list-suite-runs-malformed-version-filter-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException for a malformed nextToken, got {code!r}"
Issue-Key: iotdeviceadvisor.update-suite-definition-nonexistent-id-rejected|run#6|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: iotdeviceadvisor.create-get-delete-suite-definition-lifecycle|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotdeviceadvisor.get-suite-definition-nonexistent-id-not-found|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotdeviceadvisor.create-suite-definition-missing-root-group-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# iotdeviceadvisor: ListSuiteRuns unimplemented, UpdateSuiteDefinition not-found mapping missing, and wrong not-found / validation error message templates (4 causes)

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSuiteRuns, UpdateSuiteDefinition, GetSuiteDefinition, CreateSuiteDefinition

## Description
Four distinct root causes in the IoT Device Advisor (iotsenateservice) emulator, all surfaced by the behavioral suite (1/20 checks pass for this service). Model path used for every Smithy citation below: services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json

=== Cause 1 of 4: ListSuiteRuns is a scaffold stub and answers 501 NotImplemented ===
Failing check ids: iotdeviceadvisor.list-suite-runs-malformed-version-filter-rejected
AWS: Smithy: ListSuiteRuns is a real bound operation — GET /suiteRuns, input {suiteDefinitionId (UUID, len 12..36, httpQuery), suiteDefinitionVersion (len 2..255, httpQuery), maxResults (range 1..50), nextToken (Token, len 0..2000)}, output {suiteRunsList, nextToken}, @paginated{inputToken:nextToken, outputToken:nextToken, pageSize:maxResults}; modeled errors ValidationException (HTTP 400) and InternalServerException (HTTP 500). Recorded behavior only for the boundary itself: a syntactically invalid nextToken is refused with HTTP 400 ValidationException (recorded message, as an example: 'Invalid nextToken'), while a well-formed but unmatched suiteDefinitionVersion filter is NOT an error — it is accepted and yields an empty suiteRunsList. A fresh suite with no runs lists as an empty suiteRunsList, not an error.
Emulator: every ListSuiteRuns call answers HTTP 501 with error code NotImplemented ('operation ListSuiteRuns is scaffolded but not implemented in this emulator'). The check saw code 'NotImplemented' where it expected 'ValidationException'. Note this cause is NOT the already-filed stub cluster: the auto-filed scaffold issues in this suite name CreateKeysAndCertificate, ListTagsForResource, GetEndpoint, UpdateSuiteDefinition and UntagResource — ListSuiteRuns is not among them, and its own check was classified as a mismatch rather than a blocked stub, so nothing covers it.
Why: a client cannot enumerate suite runs at all, and cannot distinguish a bad pagination token (retry impossible, fix the token) from a server-side outage (retryable 501/500). Pagination loops and 'has this suite ever run?' logic both break, and any code that treats an unmatched version filter as an empty page instead gets an exception.
Fix: the ListSuiteRuns handler — implement listing out of the service state (filter by suiteDefinitionId and, when present, suiteDefinitionVersion, returning an empty list rather than an error on no match), plus nextToken well-formedness validation returning the modeled ValidationException (400); then drop the SCAFFOLDED marker.
Traces: services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/list_suite_runs.rs line 162 returns Scaffolded::not_implemented("ListSuiteRuns"); check iotdeviceadvisor.list-suite-runs-malformed-version-filter-rejected asserts at its line 39.

=== Cause 2 of 4: UpdateSuiteDefinition on an unknown suiteDefinitionId must be ResourceNotFoundException, not NotImplemented ===
Failing check ids: iotdeviceadvisor.update-suite-definition-nonexistent-id-rejected
AWS: Smithy: UpdateSuiteDefinition is PATCH /suiteDefinitions/{suiteDefinitionId}, input suiteDefinitionId (required, httpLabel) + suiteDefinitionConfiguration (required); its modeled error list advertises ONLY ValidationException (400) and InternalServerException (500). Recorded behavior only, and the point of the cause: real AWS nevertheless refuses an unknown id with error code ResourceNotFoundException (HTTP 404 for that shape per the model's ResourceNotFoundException binding on GetSuiteDefinition), with a message built from the request id — template 'Unable to find resource with suiteDefinitionId: <id>' (recorded example for id 1e85…/86df…). A real id still updates normally and the new suiteDefinitionName is visible to a following GetSuiteDefinition.
Emulator: the call answers HTTP 501 / NotImplemented, so the check's assertion on code ResourceNotFoundException saw 'NotImplemented'. The bare fact that the operation is a stub is already covered by the auto-filed issue naming UpdateSuiteDefinition (services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/update_suite_definition.rs); what is recorded HERE and not there is the required unknown-id error classification, which the model does not advertise and an implementer would otherwise map to ValidationException.
Why: a client that updates a suite deleted by someone else gets a non-retryable 501 (or, if guessed wrong, a 400 ValidationException) instead of the 404 ResourceNotFoundException it branches on, so 'recreate the suite' recovery paths never fire.
Fix: the UpdateSuiteDefinition handler's existence check / error mapping, alongside implementing the operation.
Traces: check iotdeviceadvisor.update-suite-definition-nonexistent-id-rejected asserts at its line 35; services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/update_suite_definition.rs.

=== Cause 3 of 4: ResourceNotFoundException message template for an unknown/deleted suiteDefinitionId ===
Failing check ids: iotdeviceadvisor.create-get-delete-suite-definition-lifecycle, iotdeviceadvisor.get-suite-definition-nonexistent-id-not-found
AWS: Smithy: GetSuiteDefinition (GET /suiteDefinitions/{suiteDefinitionId}) models ResourceNotFoundException with httpError 404. The message is built from the request — it embeds the caller's suiteDefinitionId verbatim — and the rule AWS applies is the fixed template 'Unable to find resource with suiteDefinitionId: <suiteDefinitionId>' (recorded examples: id 86df5183-3570-4da5-b41d-6b592fe6cfa8 after a delete, and id 1e8542c9-cb2a-4172-ae95-04aeb8a757da never created). The requirement is that template, not those particular ids.
Emulator: returns the right code (ResourceNotFoundException) but its own wording, 'No suite definition with suiteDefinitionId <id>' — the prefix 'Unable to find resource with ' and the colon after the member name are missing. Both reads (after delete, and never-created) diverge identically, so this is one message-construction site.
Why: clients and test harnesses that match on the AWS not-found text (log scrapers, parity assertions, regex-based retry classifiers) fail against the emulator even though the status and code are right.
Fix: the not-found error construction shared by the suite-definition handlers (GetSuiteDefinition's lookup path, and the same helper once UpdateSuiteDefinition/DeleteSuiteDefinition use it) — emit the AWS template with the requested id interpolated.
Traces: checks iotdeviceadvisor.create-get-delete-suite-definition-lifecycle (asserts at its line 66) and iotdeviceadvisor.get-suite-definition-nonexistent-id-not-found.

=== Cause 4 of 4: CreateSuiteDefinition empty-rootGroup validation message template ===
Failing check ids: iotdeviceadvisor.create-suite-definition-missing-root-group-rejected
AWS: Smithy: CreateSuiteDefinition (POST /suiteDefinitions) models ValidationException (HTTP 400) and takes suiteDefinitionConfiguration as required. Recorded behavior only for the rule: an empty rootGroup is accepted when intendedForQualification is true and refused with 400 ValidationException when it is not; the message is built from the request's violated rule and AWS's template is 'Invalid Request: rootGroup cannot be empty for non-qualification test suite'.
Emulator: refuses with the correct ValidationException but the message 'suiteDefinitionConfiguration.rootGroup cannot be empty unless intendedForQualification is true'. Same rule, different template — the 'Invalid Request: ' prefix and AWS's phrasing of the condition are absent.
Why: a client that surfaces or matches the AWS validation text for this specific rejection (the most common CreateSuiteDefinition misuse) sees a different string, so parity checks and user-facing error mapping diverge.
Fix: the CreateSuiteDefinition input validation in that operation's handler — produce AWS's template for the non-qualification empty-rootGroup rejection.
Traces: check iotdeviceadvisor.create-suite-definition-missing-root-group-rejected asserts at its line 37.

## Evidence
Suite: simparity-behavioral-aws-v1, iotdeviceadvisor 1/20 passed (19 failed; 14 of the failures are blocked by already-filed scaffold stubs and are not part of this issue).

Cause 1 — list-suite-runs-malformed-version-filter-rejected [mismatch]
  expected: HTTP 400, code ValidationException (message example 'Invalid nextToken') for nextToken='sim2real-behav-not-a-real-pagination-token'; then ListSuiteRuns with no token -> suiteRunsList == []; then suiteDefinitionVersion='does-not-exist-99' -> suiteRunsList == []
  actual:   HTTP 501, code NotImplemented — 'operation ListSuiteRuns is scaffolded but not implemented in this emulator'
  AssertionError: expected ValidationException for a malformed nextToken, got 'NotImplemented'

Cause 2 — update-suite-definition-nonexistent-id-rejected [mismatch]
  expected: code ResourceNotFoundException, message 'Unable to find resource with suiteDefinitionId: <fake-uuid>'
  actual:   code NotImplemented (HTTP 501) — 'operation UpdateSuiteDefinition is scaffolded but not implemented in this emulator'
  AssertionError: NotImplemented

Cause 3 — create-get-delete-suite-definition-lifecycle [mismatch] and get-suite-definition-nonexistent-id-not-found [mismatch]
  code ResourceNotFoundException matched in both.
  expected message 'Unable to find resource with suiteDefinitionId: 86df5183-3570-4da5-b41d-6b592fe6cfa8', got 'No suite definition with suiteDefinitionId 86df5183-3570-4da5-b41d-6b592fe6cfa8'
  expected message 'Unable to find resource with suiteDefinitionId: 1e8542c9-cb2a-4172-ae95-04aeb8a757da', got 'No suite definition with suiteDefinitionId 1e8542c9-cb2a-4172-ae95-04aeb8a757da'
  (ids are examples; the requirement is the template with the requested id interpolated)

Cause 4 — create-suite-definition-missing-root-group-rejected [mismatch]
  code ValidationException matched.
  expected message 'Invalid Request: rootGroup cannot be empty for non-qualification test suite', got 'suiteDefinitionConfiguration.rootGroup cannot be empty unless intendedForQualification is true'

## Steps to Reproduce
Boot the emulator and point a botocore iotdeviceadvisor client at it.

Cause 1:
1. create_suite_definition(suiteDefinitionConfiguration={suiteDefinitionName:<n>, rootGroup:'', devicePermissionRoleArn:<any role arn>, intendedForQualification:True}) -> note suiteDefinitionId
2. list_suite_runs(suiteDefinitionId=<id>, nextToken='sim2real-behav-not-a-real-pagination-token') -> expect 400 ValidationException; emulator answers 501 NotImplemented
3. list_suite_runs(suiteDefinitionId=<id>) -> expect {'suiteRunsList': []}
4. list_suite_runs(suiteDefinitionId=<id>, suiteDefinitionVersion='does-not-exist-99') -> expect {'suiteRunsList': []}, not an error

Cause 2:
1. update_suite_definition(suiteDefinitionId=str(uuid4()), suiteDefinitionConfiguration={suiteDefinitionName:'x', rootGroup:'', devicePermissionRoleArn:<any role arn>, intendedForQualification:True})
2. expect ResourceNotFoundException with message 'Unable to find resource with suiteDefinitionId: <that uuid>'; emulator answers 501 NotImplemented

Cause 3:
1. create_suite_definition(... intendedForQualification=True, rootGroup='') -> suiteDefinitionId
2. delete_suite_definition(suiteDefinitionId=<id>)
3. get_suite_definition(suiteDefinitionId=<id>) -> expect 404 ResourceNotFoundException, message 'Unable to find resource with suiteDefinitionId: <id>'; emulator says 'No suite definition with suiteDefinitionId <id>'
4. same call with a never-created uuid reproduces it identically

Cause 4:
1. create_suite_definition(suiteDefinitionConfiguration={suiteDefinitionName:<n>, rootGroup:'', devicePermissionRoleArn:<any role arn>, intendedForQualification:False})
2. expect 400 ValidationException, message 'Invalid Request: rootGroup cannot be empty for non-qualification test suite'; emulator returns the same code with a different template
