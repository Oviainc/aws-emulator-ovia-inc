Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifyuibuilder.create-component-duplicate-name-conflict-rejected amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected amplifyuibuilder.delete-component-state-transition-then-not-found amplifyuibuilder.tags-at-creation-visible-via-get-component amplifyuibuilder.get-component-cross-environment-isolation-rejected amplifyuibuilder.update-component-nonexistent-id-conflict-rejected amplifyuibuilder.put-metadata-flag-unknown-feature-name-rejected
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder.create-component-duplicate-name-conflict-rejected|_error_detail#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected|run#9|AssertionError|assert err["Code"] == "404", (
Issue-Key: amplifyuibuilder|GetComponent|ResourceNotFoundException
Issue-Key: amplifyuibuilder.tags-at-creation-visible-via-get-component|run#13|AssertionError|assert err["Code"] == "404", (
Issue-Key: amplifyuibuilder|UpdateComponent|InvalidParameterException
Issue-Key: amplifyuibuilder.put-metadata-flag-unknown-feature-name-rejected|run#5|AssertionError|assert code == "400", f"expected an HTTP 400 for an unrecognized feature name, got {code}"
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# AmplifyUIBuilder: 5 behavioral divergences — duplicate component name accepted, not-found/cross-app error classification wrong on Get/Delete/UpdateComponent, PutMetadataFlag still a 501 stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponent, GetComponent, DeleteComponent, UpdateComponent, PutMetadataFlag

## Description
Five distinct root causes in the AmplifyUIBuilder emulator, all found by replaying the behavioral suite against the running emulator (service is bootstrapped: 5/21 checks pass, the component CRUD path answers real responses). The service's only other filed issues are both [fixed] (bootstrap, and GetComponent timestamp/field serialization) and neither covers any cause below.

Cause 1 of 5: CreateComponent accepts a duplicate component name in the same app/environment.
AWS: CreateComponent models ResourceConflictException at HTTP 409 (Smithy: CreateComponent errors = InternalServerException 500, InvalidParameterException 400, ResourceConflictException 409, ServiceQuotaExceededException 402). Recorded behavior only for the wire shape of that refusal: a second CreateComponent naming a component name already used in the same appId+environmentName is refused with HTTP 409 whose body carries no error-type discriminator (no x-amzn-errortype, no __type), so botocore reports Error.Code as the bare status string "409"; the recorded message is, as an example, "Item name already exists". The conflict is a live-state constraint, not a malformed input: after DeleteComponent frees the name, the identical create succeeds.
Emulator: the second create SUCCEEDS with HTTP 200 and returns a second entity with a fresh id and the same name, so two components share one name in one app/environment.
Why: a client that relies on name uniqueness (create-if-absent patterns, name-keyed lookup, idempotent provisioning) silently gets duplicate components instead of a catchable 409, and its conflict-handling branch is never exercised.
Fix: CreateComponent's handler/validation plus the component store's per-(appId, environmentName) name index in this service's state — reject on an existing live name, and release the name on delete.
Traces: amplifyuibuilder.create-component-duplicate-name-conflict-rejected.

Cause 2 of 5: not-found refusals on GetComponent and DeleteComponent are tagged with a modeled error-type discriminator that AWS does not send.
AWS: the model does declare ResourceNotFoundException at HTTP 404 for GetComponent and DeleteComponent (Smithy: GetComponent/DeleteComponent errors include ResourceNotFoundException 404), but the recorded wire response for a missing component carries NO error-type discriminator, so botocore's Error.Code degrades to the bare status string "404" rather than the shape name. Recorded messages, as examples: "The item specified was not found" for GetComponent, "Unable to find item to delete" for DeleteComponent.
Emulator: the response body carries the modeled discriminator, so botocore reports Error.Code "ResourceNotFoundException" (message example: "no component <id> in app <appId> environment <env>"). The HTTP status is 404 on the GetComponent path, so the divergence is in the error classification on the wire, not the status.
Why: clients branch on Error.Code (botocore/boto3 `e.response['Error']['Code']`, and the SDK's modeled-exception classes). Against AWS the not-found arrives as an unmodeled "404" and must be handled by status; against the emulator it arrives as a typed ResourceNotFoundException. Code written against either one mis-handles the other — an `except uib.exceptions.ResourceNotFoundException` catches on the emulator and escapes on AWS.
Fix: the error serializer / not-found error channel for GetComponent and DeleteComponent in this service — emit the bare 404 envelope with no __type / x-amzn-errortype for these paths.
Traces: amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected, amplifyuibuilder.delete-component-state-transition-then-not-found, amplifyuibuilder.tags-at-creation-visible-via-get-component.

Cause 3 of 5: GetComponent cross-app isolation is answered with the wrong status class.
AWS: fetching a component id that exists, but under a DIFFERENT Amplify appId (same environmentName), is refused with HTTP 400, not 404 — recorded behavior only; the body carries no error-type discriminator (botocore Error.Code "400") and the recorded message states the item "does not belong to that app or environment". AWS distinguishes "this id is not yours in this scope" (400) from "this id does not exist" (404).
Emulator: answers HTTP 404 with Error.Code "ResourceNotFoundException" (message example: "no component <id> in app <appId> environment <env>"), collapsing the cross-scope case into plain not-found.
Why: a client that distinguishes a wrong-scope request (a bug in its own app/environment wiring, non-retryable, 4xx-validation class) from a genuinely absent resource (safe to recreate) takes the wrong branch: it will recreate a component that already exists under another app, or treat a mis-scoped call as benign.
Fix: GetComponent's handler lookup — when the id resolves to a component owned by another app/environment, return the 400 wrong-scope refusal rather than the 404 not-found path.
Traces: amplifyuibuilder.get-component-cross-environment-isolation-rejected.

Cause 4 of 5: UpdateComponent against a never-created id answers HTTP 400 instead of 404.
AWS: UpdateComponent on an id that was never created is refused with HTTP 404 — recorded behavior only, and notably NOT with the ResourceConflictException (409) its own contract models (Smithy: UpdateComponent errors = InternalServerException 500, InvalidParameterException 400, ResourceConflictException 409; no ResourceNotFoundException is modeled). The response carries no error-type discriminator; the recorded message, as an example, is "The item specified was not found". No component is created as a side effect.
Emulator: answers HTTP 400 (assertion: "UpdateComponent: expected HTTP 404, got 400"), i.e. an input-validation class status for a missing-resource condition.
Why: a client retrying or branching on status sees 400 ("my request is malformed, never retry, fix the payload") where AWS says 404 ("the target is gone — create it, or stop"). Update-or-create logic keyed on 404 never fires against the emulator.
Fix: UpdateComponent's handler — resolve the target id first and return the not-found refusal (404, bare envelope as in Cause 2) when it does not exist, instead of failing in the validation path.
Traces: amplifyuibuilder.update-component-nonexistent-id-conflict-rejected.

Cause 5 of 5: PutMetadataFlag is an unimplemented scaffold stub, so there is no featureName validation and no metadata persistence.
AWS: PutMetadataFlag stores a feature flag for an app/environment and models InvalidParameterException at HTTP 400 and UnauthorizedException at HTTP 401 (Smithy: PutMetadataFlag, PUT /app/{appId}/environment/{environmentName}/metadata/features/{featureName}, body PutMetadataFlagBody, no output members). A featureName outside the fixed set GetMetadata itself reports (autoGenerateForms / autoGenerateViews / isRelationshipSupported / isNonModelSupported / isGraphQLEnabled) is refused with HTTP 400 — recorded behavior for the wire shape, with the message, as an example, naming the invalid feature ("Feature someUnknownFeature is not valid"). A write to a KNOWN feature succeeds and the new value is visible through GetMetadata.
Emulator: every PutMetadataFlag answers HTTP 501 with Error.Code "NotImplemented" ("operation PutMetadataFlag is scaffolded but not implemented in this emulator"), so neither the rejection nor the positive write/read-back path exists.
Why: a client cannot set or round-trip any UI-builder metadata flag, and a 501 is neither the 400 it should get for a bad feature nor the success it should get for a good one; nothing it does to the request can make the call work.
Fix: implement the handler at services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/put_metadata_flag.rs (currently returns Scaffolded::not_implemented at line 178) — validate featureName against the known feature set, persist the value in this service's state, and surface it from GetMetadata. This operation is NOT among the stub issues the suite filed for this service (CreateForm, CreateTheme, ExportComponents, StartCodegenJob, ListTagsForResource).
Traces: amplifyuibuilder.put-metadata-flag-unknown-feature-name-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — amplifyuibuilder 5/21 passed (24%), 16 failed. Nine of the failures are ALREADY FILED stub blocks (CreateForm x3, CreateTheme x3, ExportComponents, StartCodegenJob, ListTagsForResource) and are excluded here. The seven remaining mismatches are the evidence below.

Cause 1 — create-component-duplicate-name-conflict-rejected:
  expected: rejection, HTTP 409, botocore Error.Code "409", message e.g. "Item name already exists"
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'entity': {'appId': 'd3t2svwf2hfab4', 'environmentName': 'staging', 'id': 'f5b12f80-7e47-417c-898a-0869418d8adb', 'name': 'Sim2realdupee3de3b838159', 'componentType': 'Text', ...}}

Cause 2 — delete-component-nonexistent-id-not-found-rejected:
  expected: Error.Code "404" (bare 404, no modeled discriminator)
  actual:   {'Message': 'no component 9fb4bb01-2f14-47c2-aef7-40ff219ed1c1 in app d0c6fzqb12piql environment staging', 'Code': 'ResourceNotFoundException'}
  tags-at-creation-visible-via-get-component (GetComponent after a successful delete):
  expected: Error.Code "404"
  actual:   {'Message': 'no component 74981193-3b69-4368-b6ff-cd980215451b in app d1m1ref289qapx environment staging', 'Code': 'ResourceNotFoundException'}
  delete-component-state-transition-then-not-found (status 404 was correct; classification was not):
  expected: unmodeled not-found envelope, message e.g. "The item specified was not found"
  actual:   {'Message': 'no component 863e6b4e-e978-4d44-bae5-800df943bb3b in app d3hsfmzlobuazu environment staging', 'Code': 'ResourceNotFoundException'}

Cause 3 — get-component-cross-environment-isolation-rejected:
  expected: HTTP 400, no modeled discriminator, message stating the item does not belong to that app or environment
  actual:   AssertionError: expected HTTP 400, got 404 ({'Message': 'no component 79d13bfd-bdfb-4bf8-9860-10262d2444f1 in app d3ujegvc5xb06e environment staging', 'Code': 'ResourceNotFoundException'})

Cause 4 — update-component-nonexistent-id-conflict-rejected:
  expected: HTTP 404 for UpdateComponent on a random uuid id
  actual:   AssertionError: UpdateComponent: expected HTTP 404, got 400

Cause 5 — put-metadata-flag-unknown-feature-name-rejected:
  expected: HTTP 400 for featureName="someUnknownFeature" (Error.Code "400"), and a successful write to featureName="autoGenerateForms" visible via GetMetadata
  actual:   AssertionError: expected an HTTP 400 for an unrecognized feature name, got NotImplemented (HTTP 501, "operation PutMetadataFlag is scaffolded but not implemented in this emulator")
  source:   services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/put_metadata_flag.rs lines 3-6 (SCAFFOLDED marker) and 178 (Err(Scaffolded::not_implemented("PutMetadataFlag")))

## Steps to Reproduce
All steps use an Amplify app created via amplify.create_app (works today) and environmentName "staging"; replay with the behavioral suite and re-run the check ids listed in trace_ids.

Cause 1: create_component(appId, environmentName='staging', componentToCreate={name: N, componentType: 'Text', properties: {label: {value: 'original'}}, variants: [], overrides: {}, bindingProperties: {}}); then call create_component again with the SAME name N. Expect HTTP 409 (botocore Error.Code "409"); the emulator returns 200 and a second entity. Then delete_component(the first id) and reissue the identical create — it must succeed.

Cause 2: create a component, then (a) delete_component with a random uuid id and (b) get_component with the id of a component that was just deleted. Inspect e.response['Error']['Code']: expect the bare "404", the emulator returns "ResourceNotFoundException".

Cause 3: create two Amplify apps A and B; create a component under A; call get_component(appId=B, environmentName='staging', id=<component id of A>). Expect HTTP 400 with the wrong-scope message; the emulator returns 404 ResourceNotFoundException. get_component against A must still succeed.

Cause 4: update_component(appId, environmentName='staging', id=<random uuid>, updatedComponent={id: <same uuid>, name: 'Ghost'}). Expect HTTP 404; the emulator returns 400. Then get_component on the same uuid to confirm nothing was created.

Cause 5: put_metadata_flag(appId, environmentName='staging', featureName='someUnknownFeature', body={'newValue': 'true'}) — expect HTTP 400; the emulator returns 501 NotImplemented. Then put_metadata_flag(..., featureName='autoGenerateForms', body={'newValue': 'false'}) and get_metadata(appId, environmentName='staging') — expect features['autoGenerateForms'] == 'false'.
