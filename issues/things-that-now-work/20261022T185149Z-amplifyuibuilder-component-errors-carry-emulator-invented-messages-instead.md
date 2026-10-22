Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifyuibuilder.update-component-nonexistent-id-conflict-rejected amplifyuibuilder.delete-component-state-transition-then-not-found amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected amplifyuibuilder.create-component-duplicate-name-conflict-rejected amplifyuibuilder.get-component-cross-environment-isolation-rejected
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder|UpdateComponent|404
Issue-Key: amplifyuibuilder|GetComponent|404
Issue-Key: amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected|run#10|AssertionError|assert err.get("Message") == "Unable to find item to delete", (
Issue-Key: amplifyuibuilder.create-component-duplicate-name-conflict-rejected|run#11|AssertionError|assert message == "Item name already exists", (
Issue-Key: amplifyuibuilder|GetComponent|400
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# AmplifyUIBuilder component errors carry emulator-invented messages instead of the service's fixed literals (the only discriminator these untyped errors have)

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
AmplifyUIBuilder's component-scoped failures are returned by real AWS WITHOUT any error-type discriminator: there is no `x-amzn-errortype` header and no `__type` in the body, so botocore's `Error.Code` degrades to the bare HTTP status string ("404", "409", "400") rather than the Smithy shape name. Every replayed check confirms the emulator already gets the HTTP status right, so the ONLY remaining machine-readable way a client can tell these failure classes apart is `Error.Message` — and AWS's message for each class is a FIXED, request-independent literal (it names no app id, environment or resource id). The emulator instead synthesises its own descriptive, request-interpolated messages from `crate::state` helpers, and additionally collapses two distinct AWS classes (component-absent on read vs. component-absent on delete) onto one shared helper. The requirement in every cause below is the constant literal per failure class, not the ids in the recorded examples.

Four causes, all in this service's component error payloads.

Cause 1 of 4: component-absent on GetComponent/UpdateComponent must say "The item specified was not found".
AWS: GetComponent models `ResourceNotFoundException` (HTTP 404) and UpdateComponent models `ResourceConflictException` (HTTP 409) (Smithy: GetComponent errors = InternalServerException 500 / InvalidParameterException 400 / ResourceNotFoundException 404; UpdateComponent errors = InternalServerException 500 / InvalidParameterException 400 / ResourceConflictException 409). The model specifies no message text, so: recorded behavior only — both operations answer an untyped HTTP 404 (botocore `Error.Code` == "404") whose `Error.Message` is the fixed literal "The item specified was not found", for an id that was never created and for an id just deleted alike. Note UpdateComponent answers 404 here even though its modeled client error is the 409 conflict.
Emulator: returns the correct untyped 404 but with a generated message interpolating the request, e.g. 'no component 1a5eea69-d525-43e0-ab0a-c85648f8eca0 in app d0mf86cg35xrtv environment staging' (example only — ids vary per run).
Why: these responses carry no error code, so a client (or SDK error-matching layer) that classifies them has nothing but the message. A client branching on AWS's literal sees an unrecognised string and misclassifies a plain not-found as an unknown failure; it can never be made to match, because the emulator's text embeds per-run ids.
Fix: the message factory for this class in the service state module, consumed by the GetComponent and UpdateComponent handlers — emit the constant literal, not a formatted string.
Traces: amplifyuibuilder.update-component-nonexistent-id-conflict-rejected, amplifyuibuilder.delete-component-state-transition-then-not-found (the post-delete GetComponent read-back).

Cause 2 of 4: component-absent on DeleteComponent must say "Unable to find item to delete" — a DIFFERENT literal from the read path.
AWS: DeleteComponent models `ResourceNotFoundException` (HTTP 404) (Smithy: DeleteComponent errors = InternalServerException 500 / InvalidParameterException 400 / ResourceNotFoundException 404; output shape Unit). Message text is recorded behavior only: an untyped HTTP 404 (`Error.Code` == "404") with the fixed literal "Unable to find item to delete" — deliberately distinct from the read path's "The item specified was not found".
Emulator: answers the correct untyped 404, but reuses the SAME shared not-found message helper as the read path, so the delete-specific class is lost entirely; observed message e.g. 'no component 75f9ed49-df00-482c-b3e6-4eaeae03b644 in app d08ghzolhb560o environment staging' (example only).
Why: AWS distinguishes "the thing you asked to delete is not here" from "the thing you asked to read is not here"; the emulator erases that distinction, so a client cannot tell a failed delete from a failed read on an untyped 404, and idempotent-delete logic keyed on the delete literal never fires.
Fix: give DeleteComponent its own not-found message rather than the shared component-not-found factory in the service state module (the handler currently builds its 404 from the same helper GetComponent uses).
Traces: amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected.

Cause 3 of 4: duplicate component name conflict must say "Item name already exists".
AWS: CreateComponent models `ResourceConflictException` (HTTP 409) (Smithy: CreateComponent errors = InternalServerException 500 / InvalidParameterException 400 / ResourceConflictException 409 / ServiceQuotaExceededException 402). Message text is recorded behavior only: creating a second component whose name is already used in the same app/environment answers an untyped HTTP 409 (`Error.Code` == "409") with the fixed literal "Item name already exists". The conflict is correctly a live-state constraint — freeing the name by deleting the original lets the identical create succeed.
Emulator: answers the correct untyped 409 but with a generated message, e.g. 'a component named Sim2realdupe1848dfa26c52 already exists in app d3bumdexpzdidh environment staging' (example only — name and app id vary per run).
Why: same untyped-error problem — a client distinguishing a name conflict from any other 409 (quota, concurrent modification) has only the message to go on and will not recognise the emulator's text.
Fix: the component name-conflict message factory in the service state module, used by the CreateComponent handler.
Traces: amplifyuibuilder.create-component-duplicate-name-conflict-rejected.

Cause 4 of 4: cross-app/environment isolation rejection must say the component "does not belong to that app or environment".
AWS: fetching a component by id from a different Amplify app (same environmentName) is rejected with HTTP 400 — the modeled `InvalidParameterException` (Smithy: GetComponent errors include InvalidParameterException 400, "An invalid or out-of-range value was supplied for the input parameter"). Message text is recorded behavior only: an untyped HTTP 400 (`Error.Code` == "400") whose message contains the fixed phrase "does not belong to that app or environment" — a constant phrase that names no ids.
Emulator: returns the correct untyped 400 and the right classification (wrong-scope, not not-found), but with an interpolated message, e.g. 'component a3f9a0e6-250b-4046-831f-21de76c8bf3a does not belong to app d1wedgekq24ngi environment staging' (example only). The phrase a client matches on, "to that app or environment", never appears because the ids are substituted in its place.
Why: a client asserting on, or logging/alerting off, the isolation phrase sees an unrecognised 400 and cannot separate a scope violation from an ordinary malformed-parameter rejection.
Fix: the wrong-scope message factory in the service state module, consumed by the GetComponent (and DeleteComponent) wrong-scope branch.
Traces: amplifyuibuilder.get-component-cross-environment-isolation-rejected.

No open issue covers these; the only open amplifyuibuilder issue is the ListTagsForResource scaffold stub (20261022T142124Z-amplifyuibuilder-listtagsforresource-stub.md), which is unrelated. The DeleteForm / DeleteTheme / UpdateForm / UpdateTheme / ListTagsForResource stub failures in this suite run are already filed and are excluded here.

## Evidence
All five failing checks reach the message assertion, i.e. the HTTP status and the untyped `Error.Code` already match AWS; only `Error.Message` diverges.

Cause 1 — expected `Error.Message` == "The item specified was not found" (untyped 404):
  UpdateComponent on a never-created id: AssertionError: UpdateComponent: expected the unmodeled not-found body, got 'no component 1a5eea69-d525-43e0-ab0a-c85648f8eca0 in app d0mf86cg35xrtv environment staging'
  GetComponent after a successful delete: AssertionError: expected the not-found message, got {'Message': 'no component 50dabdc6-0406-44b7-8618-3cec4c5184b5 in app d22suz8yjq0t22 environment staging', 'Code': '404'}

Cause 2 — expected `Error.Message` == "Unable to find item to delete" (untyped 404):
  AssertionError: expected the exact not-found message, got {'Message': 'no component 75f9ed49-df00-482c-b3e6-4eaeae03b644 in app d08ghzolhb560o environment staging', 'Code': '404'}
  (Code '404' asserted OK first; the emulator reuses the read path's message, so the delete-specific literal never appears.)

Cause 3 — expected `Error.Message` == "Item name already exists" (untyped 409):
  AssertionError: expected the rejection message to be 'Item name already exists', got 'a component named Sim2realdupe1848dfa26c52 already exists in app d3bumdexpzdidh environment staging'
  (Error.Code '409' asserted OK first; the original component was correctly left untouched, the control create succeeded.)

Cause 4 — expected `Error.Message` to contain "does not belong to that app or environment" (untyped 400):
  AssertionError: expected the cross-app-isolation message, got {'Message': 'component a3f9a0e6-250b-4046-831f-21de76c8bf3a does not belong to app d1wedgekq24ngi environment staging', 'Code': '400'}

Source of the generated text (citations, not a patch): services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs lines 1023-1029 (`component_not_found`, formats "no component {id} in app {app_id} environment {environment_name}"), lines 1034-1044 (`component_wrong_scope`, formats "component {id} does not belong to app {app_id} environment {environment_name}"), lines 1046-1059 (component name-conflict factory, formats "a component named {name} already exists in app {app_id} environment {environment_name}"). These are called from services/.../src/ops/get_component.rs lines 162-179 and services/.../src/ops/delete_component.rs lines 183-200 — the delete handler calls the same `component_not_found` helper as the read handler, which is how cause 2's distinct class is lost.

All generated/per-run values above (uuids, app ids, component names) are examples; the requirement is the constant literal per failure class.

## Steps to Reproduce
1. Create an Amplify app (amplify CreateApp) and use environmentName "staging".
2. Cause 1: POST CreateComponent for a Text component, then PATCH UpdateComponent with a random uuid id that was never created. Observe HTTP 404 with no x-amzn-errortype/__type; inspect the body message. Repeat with GET GetComponent on that same missing id, and on the id of a component you just deleted. Expect message "The item specified was not found" in all three; the emulator returns "no component <id> in app <appId> environment staging".
3. Cause 2: DELETE DeleteComponent with a random uuid id that was never created. Expect HTTP 404 with message "Unable to find item to delete"; the emulator returns the same read-path "no component <id> in app ..." text.
4. Cause 3: CreateComponent with name N, then CreateComponent again with the identical name N in the same app/environment. Expect HTTP 409 with message "Item name already exists"; the emulator returns "a component named N already exists in app <appId> environment staging".
5. Cause 4: create a component in app A, then GetComponent for that component's id under a DIFFERENT app B with the same environmentName. Expect HTTP 400 whose message contains "does not belong to that app or environment"; the emulator returns "component <id> does not belong to app <appIdB> environment staging".
6. Re-replay: amplifyuibuilder.update-component-nonexistent-id-conflict-rejected, amplifyuibuilder.delete-component-state-transition-then-not-found, amplifyuibuilder.delete-component-nonexistent-id-not-found-rejected, amplifyuibuilder.create-component-duplicate-name-conflict-rejected, amplifyuibuilder.get-component-cross-environment-isolation-rejected.
