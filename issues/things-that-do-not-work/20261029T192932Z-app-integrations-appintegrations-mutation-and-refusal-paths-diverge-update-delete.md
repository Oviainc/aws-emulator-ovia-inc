Issue-Source: trace-sweep-divergence
Issue-Trace: appintegrations-application-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appintegrations-application-error-paths appintegrations-event-integration-error-paths appintegrations-application-lifecycle appintegrations-application-full-config appintegrations-data-integration-error-paths
Issue-Shape-Id: com.amazonaws.appintegrations#AmazonAppIntegrationService
Issue-Key: app-integrations|CreateApplication|4xx->4xx|$.Message
Issue-Key: app-integrations|CreateApplication|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: app-integrations|CreateApplication|4xx->4xx|status
Issue-Key: app-integrations|CreateEventIntegration|4xx->4xx|$.Message
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Description
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.ApplicationConfig.ContactHandling.Scope
Issue-Key: app-integrations|GetApplication|2xx->2xx|$.Permissions[*]
Issue-Key: app-integrations|GetApplication|4xx->2xx|$
Issue-Key: app-integrations|GetApplication|4xx->2xx|$.Message
Issue-Key: app-integrations|GetApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: app-integrations|GetApplication|4xx->2xx|status
Issue-Key: app-integrations|ListDataIntegrationAssociations|2xx->4xx|$.DataIntegrationAssociations
Issue-Key: app-integrations|ListDataIntegrationAssociations|2xx->4xx|status
Issue-Related: 20261029T192932Z-app-integrations-appintegrations-read-paths-omit-server-defaulted-members-on.md
Issue-Commit: e7e2240280b5af8861aa38841849f2941d902f6c

# appintegrations mutation and refusal paths diverge: update/delete never applied, list 404s, duplicate-name errors wrong

> _Note: this issue was identified against the code base at commit `e7e2240280b5af8861aa38841849f2941d902f6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
SIX net-new causes on the AppIntegrations emulator, all in the write/refusal plane: two duplicate-name refusals answer the wrong class or wording, application updates and deletes are never reflected in state, and ListDataIntegrationAssociations refuses an existing integration that simply has no associations. No open appintegrations issue covers any of these (prior appintegrations issues — bootstrap, the BadRequestException/ResourceNotFoundException wording issue, and the CreateApplication/CreateDataIntegration stub issues — are all resolved). Causes 3-5 are read-backs that expose UpdateApplication and DeleteApplication being scaffold stubs; the auto-filed 'unimplemented scaffold stub' issues for those two operations cover the 501 answers themselves, NOT these recorded state divergences, which are what a client actually observes.

Cause 1 of 6 — CreateApplication refuses a duplicate Name+Namespace with the wrong status and error class.
AWS: recorded behavior — a CreateApplication whose Name is already in use in the same Namespace is refused with HTTP 400, x-amzn-errortype InvalidRequestException, and the message is entirely FIXED text with no variable parts: "Application name is invalid. Name already in use." (It deliberately names neither the application nor the namespace.) Smithy models both InvalidRequestException (HTTP 400, service-common) and DuplicateResourceException (HTTP 409) on CreateApplication, so the model alone does not pick one; the recording does.
Emulator: HTTP 409, x-amzn-errortype DuplicateResourceException, message "An application named <Name> already exists in namespace <Namespace>." — an emulator-authored template interpolating the request's Name and Namespace.
Why: a client branching on the 4xx class or on the error code takes its conflict/retry path instead of its invalid-input path, and error-text assertions never match.
Fix: the CreateApplication handler's duplicate-name branch — raise InvalidRequestException (400) with AWS's fixed sentence.
Traces: appintegrations-application-error-paths#task#4. Code: services/service-com-amazonaws-appintegrations-amazonappintegrationservice/src/ops/create_application.rs:211-225.

Cause 2 of 6 — CreateEventIntegration's duplicate-name message is emulator-authored.
AWS: recorded behavior — HTTP 409 DuplicateResourceException (status and code already match) with message "An EventIntegration already exists with name: <Name>." Fixed text: "An EventIntegration already exists with name: " and the trailing period; the single variable part is the request's Name member.
Emulator: "An event integration named <Name> already exists." — same information, different wording.
Why: the message is part of the contract; clients and test suites that match on AWS's sentence fail against the emulator.
Fix: the CreateEventIntegration handler's duplicate branch message template.
Traces: appintegrations-event-integration-error-paths#task#4. Code: src/ops/create_event_integration.rs:186-196.

Cause 3 of 6 — an UpdateApplication that changes Description is not reflected by GetApplication.
AWS: Smithy UpdateApplication (PATCH /applications/{Arn}, empty output) persists the members the request carries; Description targets Description (@length 0..1000). Recorded behavior: after UpdateApplication with a new Description, GetApplication reports the NEW Description and a moved LastModifiedTime.
Emulator: GetApplication still reports the Description that CreateApplication stored — the update left state untouched (UpdateApplication::handle is still the scaffold's not_implemented).
Why: an update a client issued and the service accepted silently does not take; read-after-write is wrong for every mutable member of an application.
Fix: implement the UpdateApplication handler so it writes the request's members onto the stored ApplicationRecord and advances LastModifiedTime.
Traces: appintegrations-application-lifecycle#task#5. Code: src/ops/update_application.rs:166-184 (returns Scaffolded::not_implemented).

Cause 4 of 6 — the same unapplied update loses ApplicationConfig.ContactHandling.Scope and Permissions.
AWS: Smithy ApplicationConfig.ContactHandling.Scope targets the ContactHandlingScope enum; Permissions targets PermissionList (@length 0..150). Recorded behavior: after UpdateApplication sets Scope and a second permission, GetApplication reports the updated Scope and the full updated Permissions list.
Emulator: GetApplication reports the Scope stored at create time and only the first permission — the second Permissions entry is absent.
Why: a client reconfiguring an application's contact-handling scope or widening its permissions sees the old configuration, so authorization-relevant state read back from the emulator is stale.
Fix: same handler as cause 3 — the update must replace ApplicationConfig and Permissions on the stored record.
Traces: appintegrations-application-full-config#task#4.

Cause 5 of 6 — DeleteApplication does not remove the application: GetApplication keeps answering 200.
AWS: Smithy DeleteApplication (DELETE /applications/{Arn}) removes the resource; a subsequent GetApplication is refused with HTTP 404 ResourceNotFoundException whose message is "Could not find Application with identifier: <identifier>." — fixed text "Could not find Application with identifier: " plus the identifier the request sent (the ARN or UUID), plus the period.
Emulator: GetApplication keeps answering HTTP 200 with the full application record indefinitely (the fixture polled 29 times waiting for the 404 and never got it); DeleteApplication::handle is still the scaffold's not_implemented.
Why: deletion is unobservable — teardown never converges, a deleted application stays listable and readable, and its Name+Namespace stays occupied so a recreate under the same name hits the duplicate path of cause 1.
Fix: implement the DeleteApplication handler so it drops the ApplicationRecord (and its ClientToken index entry) from the store; the existing crate::state::application_not_found wording is already correct for the follow-up 404.
Traces: appintegrations-application-full-config#teardown#6 (also appintegrations-application-lifecycle#teardown#11). Code: src/ops/delete_application.rs:148-166.

Cause 6 of 6 — ListDataIntegrationAssociations answers 404 for an existing DataIntegration with no associations.
AWS: Smithy ListDataIntegrationAssociations (GET /dataIntegrations/{DataIntegrationIdentifier}/associations), output DataIntegrationAssociations + NextToken; DataIntegrationIdentifier is documented as a unique identifier for the DataIntegration and constrained only to a non-blank string. Recorded behavior: for an existing DataIntegration that has no associations the service answers HTTP 200 with DataIntegrationAssociations: [] (an empty page, not a refusal).
Emulator: HTTP 404 ResourceNotFoundException and no DataIntegrationAssociations member. Hypothesis: the handler resolves the identifier through crate::state::data_integration_id, which matches only the minted Id or the full stored ARN, so an identifier form AWS accepts (notably the integration's Name) resolves to nothing and the existing integration is reported as not found.
Why: a client enumerating associations of an integration it just created gets a hard 404 instead of an empty list, so pagination loops and cleanup code abort on a resource that exists.
Fix: the ListDataIntegrationAssociations handler's identifier resolution in state (accept every identifier form the service hands out), keeping the empty-page answer for a resolved integration with zero associations.
Traces: appintegrations-data-integration-error-paths#task#2. Code: src/ops/list_data_integration_associations.rs:163-175, src/state.rs:570-578.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, 53 steps, 27 passed, 26 mismatched).

Cause 1 — appintegrations-application-error-paths#task#4, CreateApplication: status expected 400, got 409; $headers.x-amzn-errortype expected 'InvalidRequestException', got 'DuplicateResourceException'; $.Message expected 'Application name is invalid. Name already in use.', got 'An application named sim2real-probe-appintegrations-app-err-<suffix> already exists in namespace sim2real-probe-ns-err-<suffix>.' (the recorded generated names are per-run values, not part of the contract).

Cause 2 — appintegrations-event-integration-error-paths#task#4, CreateEventIntegration, 409 vs 409: $.Message expected 'An EventIntegration already exists with name: <Name>.', got 'An event integration named <Name> already exists.'

Cause 3 — appintegrations-application-lifecycle#task#5, GetApplication, 200 vs 200: $.Description expected 'Updated probe application', got the create-time description 'Probe application for lifecycle testing'.

Cause 4 — appintegrations-application-full-config#task#4, GetApplication, 200 vs 200: $.ApplicationConfig.ContactHandling.Scope expected 'PER_CONTACT', got 'CROSS_CONTACTS'; $.Permissions[1] expected 'Case.Write', got None.

Cause 5 — appintegrations-application-full-config#teardown#6 (and appintegrations-application-lifecycle#teardown#11), GetApplication: status expected 404, got 200; $.Message expected 'Could not find Application with identifier: <application ARN>.', got None; poll gave up after 29 attempts with error code None not in ['ResourceNotFoundException'], the body still carrying the full application record.

Cause 6 — appintegrations-data-integration-error-paths#task#2, ListDataIntegrationAssociations: status expected 200, got 404; $.DataIntegrationAssociations expected [], got None.

Model evidence: smithy_query(operation="CreateApplication,CreateEventIntegration,UpdateApplication,ListDataIntegrationAssociations") and smithy_query(shape="ApplicationConfig,ContactHandling,PermissionList") on services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json.

## Steps to Reproduce
1. Cause 1: replay appintegrations-application-error-paths step task#4 — CreateApplication twice with the same Name and Namespace; the second call must be 400 InvalidRequestException 'Application name is invalid. Name already in use.', the emulator answers 409 DuplicateResourceException with its own sentence.
2. Cause 2: replay appintegrations-event-integration-error-paths step task#4 — CreateEventIntegration twice with the same Name; compare the Message against 'An EventIntegration already exists with name: <Name>.'
3. Cause 3: replay appintegrations-application-lifecycle step task#5 — CreateApplication, UpdateApplication with a new Description, then GetApplication; the Description must be the updated one.
4. Cause 4: replay appintegrations-application-full-config step task#4 — UpdateApplication setting ApplicationConfig.ContactHandling.Scope=PER_CONTACT and Permissions=[Case.Read, Case.Write], then GetApplication; both must be reflected.
5. Cause 5: replay appintegrations-application-full-config step teardown#6 (and appintegrations-application-lifecycle teardown#11) — DeleteApplication, then poll GetApplication; it must settle on 404 ResourceNotFoundException.
6. Cause 6: replay appintegrations-data-integration-error-paths step task#2 — CreateDataIntegration, then ListDataIntegrationAssociations for it with no associations created; must be 200 with DataIntegrationAssociations: [].
7. Re-run replay_wire_corpus() after the fix; all six steps must pass.
