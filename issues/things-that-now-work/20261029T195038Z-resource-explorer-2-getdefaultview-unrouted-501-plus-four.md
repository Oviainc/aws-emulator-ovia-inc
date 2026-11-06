Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2.default-view-association-overwrites-previous resource-explorer-2.delete-default-view-clears-default-pointer resource-explorer-2.disassociate-default-view-when-none-set-is-noop resource-explorer-2.list-resources-without-view-or-default-succeeds resource-explorer-2.search-explicit-view-overrides-default resource-explorer-2.create-view-invalid-name-pattern-rejected resource-explorer-2.delete-index-transitions-to-not-found resource-explorer-2.view-create-get-list-delete-lifecycle resource-explorer-2.view-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|GetDefaultView|NotImplemented
Issue-Key: resource-explorer-2.create-view-invalid-name-pattern-rejected|run#6|AssertionError|assert error == ("UnauthorizedException", "Unauthorized"), f"observed error={error!r}"
Issue-Key: resource-explorer-2.delete-index-transitions-to-not-found|run#9|AssertionError|assert second_delete_code == "ValidationException", (
Issue-Key: resource-explorer-2.view-create-get-list-delete-lifecycle|run#26|AssertionError|assert get_error == expected, f"expected GetView on deleted view to fail with {expected!r}, got {get_error!r}"
Issue-Key: resource-explorer-2.view-name-conflict-rejected|run#9|AssertionError|assert error == expected, f"expected {expected!r} for a duplicate ViewName, got {error!r}"
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Resource Explorer 2: GetDefaultView unrouted (501) plus four wrong refusal classes/messages on CreateView, GetView and DeleteIndex

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDefaultView

## Description
FIVE root causes (Cause 1 of 5 .. Cause 5 of 5), all in the Resource Explorer 2 service crate (services/service-com-amazonaws-resourceexplorer2-resourceexplorer). A fixer must close all five and re-replay every check id listed per cause before submitting.

Note on already-filed work: the open issue 20261029T191823Z-resource-explorer-2-createview-refuses-with-400-validationexception-instead-of-creating.md covers CreateView refusing a well-formed request because the Region holds no index (the `live_index` precondition). It does NOT cover any of the five causes below. The open issue 20261029T144855Z (ListResources scaffold stub) and the stub issues the replay report filed for BatchGetView, UpdateView, UpdateIndexType and CreateResourceExplorerSetup are likewise separate and excluded here.

=== Cause 1 of 5: GetDefaultView is not routed at all — every call answers 501 NotImplemented ===
Failing checks: resource-explorer-2.default-view-association-overwrites-previous, resource-explorer-2.delete-default-view-clears-default-pointer, resource-explorer-2.disassociate-default-view-when-none-set-is-noop, resource-explorer-2.list-resources-without-view-or-default-succeeds, resource-explorer-2.search-explicit-view-overrides-default.
AWS: Smithy `com.amazonaws.resourceexplorer2#GetDefaultView` — HTTP POST /GetDefaultView, input Unit, success 200 with `GetDefaultViewOutput{ViewArn: String}` (ViewArn is OPTIONAL), modeled errors ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400), plus the service-common AccessDeniedException/InternalServerException. Recorded behavior (from the checks' own documented expectations): with no default view set for the Region the call returns 200 with the `ViewArn` key ABSENT — it does not raise — and after AssociateDefaultView it returns 200 with the most recently associated view's ARN.
Emulator: HTTP 501, error code `NotImplemented`, message `operation POST /GetDefaultView is not implemented in this emulator`. This is the router's catch-all, not a scaffold stub: there is no handler module for the operation (a read of services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/get_default_view.rs reports the file does not exist), which is why the replay report did not mark these checks as stub-blocked.
Why: the Region default view is the pivot of the whole service — a client that reads the default view to scope Search/ListResources, or that saves and restores it around a change, cannot run at all. Worse, `NotImplemented` is not in the operation's modeled error set, so SDK error handling for this operation never matches it and the failure surfaces as an unmodeled protocol error. Five of this suite's checks never get past the first default-view read.
Fix: a new handler module `ops/get_default_view.rs` plus its registration in the service's operation router/mod (the generated `EmulatedService` wiring in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/mod.rs), reading the per-Region default-view pointer from services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/state.rs. It must return 200 with `ViewArn` omitted when the Region has no default, and 200 with the pointer when it has one.
Traces: the five check ids above; each fails on the first `GetDefaultView` call.

=== Cause 2 of 5: CreateView with an over-long ViewName is refused in the wrong class (400 ValidationException instead of 401 UnauthorizedException) ===
Failing check: resource-explorer-2.create-view-invalid-name-pattern-rejected.
AWS: Smithy `CreateView.ViewName` is @required and @pattern `^[a-zA-Z0-9\-]{1,64}$`; the operation models UnauthorizedException (401, "The principal making the request isn't permitted to perform the operation") and ValidationException (400). Recorded behavior only: for a ViewName longer than 64 characters real AWS refuses the call with error code `UnauthorizedException` and message `Unauthorized` — authorization resolves the would-be view ARN before the service's own length validation runs, so a name that cannot produce a matchable ARN surfaces as an authorization failure, not a parameter-validation failure. No view is persisted under the rejected name, and a compliant name in the same Region still succeeds.
Emulator: HTTP 400, code `ValidationException`, message `1 validation error detected: Value '<the submitted ViewName>' at 'viewName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\-]{1,64}$`. Fixed text here is the framework's generic constraint-report template; the variable parts are the submitted name, the member name and the pattern. AWS's required refusal for this class is code `UnauthorizedException` with message exactly `Unauthorized` (no variable parts).
Why: a client distinguishes "my input is malformed" (400 ValidationException, fix the name) from "I am not permitted" (401 UnauthorizedException, fix credentials/policy). The emulator inverts that signal, so parity tests and retry/credential-refresh logic that key on 401 UnauthorizedException take the wrong branch, and the pre-handler framework validation shadows a refusal AWS makes in the authorization layer.
Fix: CreateView's validation path — the modeled-constraint pre-check for `ViewName` must not be the terminal refusal for this operation; the authorization-class refusal belongs ahead of it in `CreateView::handle` / the request walk's validation stage in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/create_view.rs.
Traces: resource-explorer-2.create-view-invalid-name-pattern-rejected.

=== Cause 3 of 5: re-deleting a terminal index returns 404 ResourceNotFoundException instead of 400 ValidationException naming the DELETED state ===
Failing check: resource-explorer-2.delete-index-transitions-to-not-found.
AWS: Smithy `DeleteIndex` models ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); `IndexState` includes the terminal value `DELETED` and the operation's docs describe deletion as an asynchronous background task. Recorded behavior only: after a DeleteIndex settles, the index record persists TERMINALLY rather than disappearing — a second DeleteIndex on the same ARN is refused with code `ValidationException` and a message that NAMES the `DELETED` state (fixed text is AWS's wording for the state-conflict; the variable parts are the index ARN/state token), not with ResourceNotFoundException.
Emulator: HTTP 404, code `ResourceNotFoundException`, message `Index not found: <index arn>` (fixed text `Index not found: ` + the ARN from the request). Root cause in state: `settle()` removes the index entry outright, and `DeleteIndex::handle` (services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/delete_index.rs lines 169-180) then finds no entry at all.
Why: the two classes mean different things to a client — 404 says "this index never existed / is gone, you may create one", whereas AWS's 400 ValidationException naming DELETED says "the record exists in a terminal state". Cleanup and idempotency logic that treats 404 as "already cleaned up, proceed" versus a state conflict diverges, and callers polling GetIndex for the terminal `DELETED` state (which AWS keeps reporting) see the resource vanish instead.
Fix: the index lifecycle in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/state.rs (`settle` / `IndexLifecycle`) must retain a terminal `DELETED` entry instead of dropping it, and `DeleteIndex::handle` must refuse a call against such an entry with the modeled 400 ValidationException whose message names the DELETED state.
Traces: resource-explorer-2.delete-index-transitions-to-not-found.

=== Cause 4 of 5: GetView on a deleted/unresolvable view ARN returns 404 ResourceNotFoundException instead of 401 UnauthorizedException / "Unauthorized" ===
Failing check: resource-explorer-2.view-create-get-list-delete-lifecycle.
AWS: Smithy `GetView` models UnauthorizedException (401), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Recorded behavior only: a GetView naming a view ARN that does not resolve to a real, accessible view (here, one just deleted via DeleteView) is refused with code `UnauthorizedException` and message exactly `Unauthorized` — the ARN cannot be matched by authorization, so the refusal is the authorization class with no variable parts in the message. (The same AWS rule is what the already-filed BatchGetView check expects, so it is the service's general shape for unresolvable view ARNs.)
Emulator: HTTP 404, code `ResourceNotFoundException`, message `View not found: <view arn>` — fixed text `View not found: ` plus the ARN taken from the request. Source: `GetView::handle`, services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/get_view.rs lines 166-173.
Why: a client cannot distinguish "gone" from "not permitted" the way AWS does; code paths and parity assertions keyed on 401 UnauthorizedException for an unresolvable view ARN take the wrong branch, and an emulator that reports 404 also leaks the existence/absence of a view that AWS refuses to confirm.
Fix: `GetView::handle` in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/get_view.rs — the unresolved-ARN branch must return the modeled 401 UnauthorizedException with message `Unauthorized`.
Traces: resource-explorer-2.view-create-get-list-delete-lifecycle (CreateView → GetView → ListViews → DeleteView → GetView).

=== Cause 5 of 5: duplicate-ViewName ConflictException carries an emulator-invented message instead of AWS's wording ===
Failing check: resource-explorer-2.view-name-conflict-rejected.
AWS: Smithy `ConflictException` (409, Message @required) is documented for exactly this case — "If you attempted to create a view, then the request failed because either you specified parameters that didn’t match the original request, or you attempted to create a view with a name that already exists in this Amazon Web Services Region." Recorded behavior: the code is `ConflictException` and the message is the fixed, wholly static text `The request failed because either the request parameters didn't match to the original request or a view with the specified name already exists in this AWS Region.` — it has NO variable parts: no view name, no Region, no ARN.
Emulator: correct code `ConflictException`, but message `A view named <ViewName> already exists in Region <region>: <existing view arn>` — emulator-invented fixed text plus three variable parts (the ViewName from the request, the Region from the SigV4 scope, and the existing view's ARN from state). Source: `CreateView::handle`, services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/create_view.rs lines 259-266 (and the sibling ClientToken-mismatch branch at lines 243-252 invents text for the same error class).
Why: the message is part of the contract. Clients, SDK error-matchers, snapshot/parity tests and support tooling that key on AWS's wording do not match, and the emulator additionally discloses the ARN of an existing view that AWS deliberately does not reveal in this refusal.
Fix: `CreateView::handle`'s duplicate-name and idempotency-token-mismatch ConflictException branches in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/create_view.rs must emit AWS's fixed message text for the view-conflict class verbatim, with no request- or state-derived substitutions.
Traces: resource-explorer-2.view-name-conflict-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. Overall 7/23 passed; resource-explorer-2 6/21 (15 failed). All evidence below is quoted from that replay run; none of it depends on a per-run value (request ids, clocks and generated view/index ids appear only as the variable parts noted per cause).

Cause 1 — GetDefaultView unrouted (5 checks, identical exception):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetDefaultView operation: operation POST /GetDefaultView is not implemented in this emulator
  raised in resource-explorer-2.default-view-association-overwrites-previous, .delete-default-view-clears-default-pointer, .disassociate-default-view-when-none-set-is-noop, .list-resources-without-view-or-default-succeeds, .search-explicit-view-overrides-default.
  Expected: HTTP 200 + GetDefaultViewOutput; `ViewArn` key ABSENT when no default is set (the checks read it with `rex.get_default_view().get("ViewArn")` precisely because AWS does not raise), and equal to the most recently associated ARN after AssociateDefaultView.
  Handler absence: read of services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/get_default_view.rs → "is not a file under the checkout". Model: smithy_query digest lists GetDefaultView → POST /GetDefaultView, input Unit, output GetDefaultViewOutput, errors [ResourceNotFoundException, ThrottlingException, ValidationException].

Cause 2 — CreateView over-long name:
  AssertionError: observed error=('ValidationException', "1 validation error detected: Value 'sim2real-rexbadname-<generated>xxxxxxxx…' at 'viewName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]{1,64}$")
  Expected: ('UnauthorizedException', 'Unauthorized')  [HTTP 401]. Actual: HTTP 400 ValidationException. The submitted name is a >64-char name generated by the check; only its length matters.

Cause 3 — second DeleteIndex on a terminal index:
  AssertionError: expected ValidationException re-deleting a terminal index, got 'ResourceNotFoundException' ('Index not found: arn:aws:resource-explorer-2:<region>:<account>:index/<index-id>')
  Expected: HTTP 400 ValidationException whose message contains "DELETED". Actual: HTTP 404 ResourceNotFoundException. Code path: ops/delete_index.rs:169-180 (`guard.indexes.values_mut().find(...)` → ResourceNotFoundException "Index not found: {arn}") after state.rs `settle()` has removed the entry.

Cause 4 — GetView on a deleted view:
  AssertionError: expected GetView on deleted view to fail with ('UnauthorizedException', 'Unauthorized'), got ('ResourceNotFoundException', 'View not found: arn:aws:resource-explorer-2:<region>:<account>:view/<name>/<view-id>')
  Expected: HTTP 401 UnauthorizedException, message `Unauthorized`. Actual: HTTP 404 ResourceNotFoundException. Code path: ops/get_view.rs:166-173.

Cause 5 — duplicate ViewName:
  AssertionError: expected ('ConflictException', "The request failed because either the request parameters didn't match to the original request or a view with the specified name already exists in this AWS Region.") for a duplicate ViewName, got ('ConflictException', 'A view named <ViewName> already exists in Region <region>: arn:aws:resource-explorer-2:<region>:<account>:view/<ViewName>/<view-id>')
  Message parts — AWS: 100% fixed text, zero variable parts. Emulator: fixed text "A view named … already exists in Region …: …" + variable parts ViewName (request), Region (SigV4 scope), existing ViewArn (state). Code path: ops/create_view.rs:259-266.

## Steps to Reproduce
Prerequisite for every step: boot the emulator from this checkout and point botocore/CLI at it with service name `resource-explorer-2`, then `CreateIndex` in the signing Region and poll `GetIndex` until State==ACTIVE (the shared setup every check performs).

Cause 1 (GetDefaultView unrouted):
  1. Call `GetDefaultView` with no arguments.
     Observed: HTTP 501, code NotImplemented, "operation POST /GetDefaultView is not implemented in this emulator".
     Expected: HTTP 200, body with no `ViewArn` key.
  2. `CreateView --view-name <a>`, `AssociateDefaultView --view-arn <arnA>`, then `GetDefaultView` → expect 200 with ViewArn == arnA; repeat with a second view and expect the pointer to be overwritten.
  3. Re-replay: resource-explorer-2.default-view-association-overwrites-previous, .delete-default-view-clears-default-pointer, .disassociate-default-view-when-none-set-is-noop, .list-resources-without-view-or-default-succeeds, .search-explicit-view-overrides-default.

Cause 2 (CreateView over-long ViewName):
  1. `CreateView --view-name <any name longer than 64 characters>`.
     Observed: 400 ValidationException, "1 validation error detected: Value '<name>' at 'viewName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]{1,64}$".
     Expected: 401 UnauthorizedException, message "Unauthorized".
  2. Confirm no view was persisted (`ListViews` contains no ARN carrying that name) and that a compliant name still returns 200.
  3. Re-replay: resource-explorer-2.create-view-invalid-name-pattern-rejected.

Cause 3 (terminal index re-delete):
  1. `DeleteIndex --arn <indexArn>` → 200 with State DELETING/DELETED.
  2. Poll `GetIndex` until it reports terminal DELETED (or 404).
  3. `DeleteIndex --arn <same arn>` again.
     Observed: 404 ResourceNotFoundException "Index not found: <arn>".
     Expected: 400 ValidationException whose message names the DELETED state.
  4. Re-replay: resource-explorer-2.delete-index-transitions-to-not-found.

Cause 4 (GetView after DeleteView):
  1. `CreateView --view-name <n>` → capture ViewArn; `GetView --view-arn <arn>` → 200.
  2. `DeleteView --view-arn <arn>`, then `GetView --view-arn <arn>`.
     Observed: 404 ResourceNotFoundException "View not found: <arn>".
     Expected: 401 UnauthorizedException, message "Unauthorized".
  3. Re-replay: resource-explorer-2.view-create-get-list-delete-lifecycle.

Cause 5 (duplicate ViewName message):
  1. `CreateView --view-name dup-name` → 200.
  2. `CreateView --view-name dup-name` again in the same Region.
     Observed: 409 ConflictException, "A view named dup-name already exists in Region <region>: <existing arn>".
     Expected: 409 ConflictException with the fixed message "The request failed because either the request parameters didn't match to the original request or a view with the specified name already exists in this AWS Region." (no name, Region or ARN interpolated).
  3. Re-replay: resource-explorer-2.view-name-conflict-rejected.
