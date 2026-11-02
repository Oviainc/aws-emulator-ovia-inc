Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.base64-blob-property-persists cloudcontrol.base64-blob-property-round-trip cloudcontrol.cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native cloudcontrol.delete-resource-then-not-found cloudcontrol.delete-then-recreate-succeeds cloudcontrol.e2e-create-update-observe-delete-flow cloudcontrol.empty-string-vs-absent-property-preserved cloudcontrol.get-resource-nonexistent-returns-not-found cloudcontrol.list-resource-requests-chronological-order cloudcontrol.list-resource-requests-resource-type-filter-preserves-order cloudcontrol.numeric-precision-property-round-trip cloudcontrol.progress-event-request-tokens-distinct-and-records-retained cloudcontrol.same-identifier-different-types-isolated cloudcontrol.timestamp-property-precision-preserved cloudcontrol.timestamp-property-preserved cloudcontrol.timestamp-property-preserved-exact-format cloudcontrol.concurrent-create-same-identifier-race cloudcontrol.concurrent-delete-and-update-same-identifier-race cloudcontrol.concurrent-delete-during-update-race-single-winner cloudcontrol.concurrent-delete-same-identifier-race cloudcontrol.concurrent-update-same-identifier-race cloudcontrol.delete-resource-during-in-progress-create-conflict cloudcontrol.update-during-in-progress-create-conflict cloudcontrol.client-token-conflict-different-params cloudcontrol.duplicate-client-token-conflict-rejected cloudcontrol.server-side-unique-request-token-per-operation cloudcontrol.server-side-unique-request-token-per-operation-dup-removed cloudcontrol.get-resource-request-status-bogus-token cloudcontrol.get-resource-request-status-invalid-token-rejected cloudcontrol.get-resource-request-status-unknown-token-rejected cloudcontrol.request-token-ttl-expiry-status-query cloudcontrol.create-only-property-mutation-rejected cloudcontrol.patch-multi-op-atomicity-no-partial-apply cloudcontrol.update-resource-immutable-property-not-updatable cloudcontrol.update-resource-invalid-client-token-format-rejected cloudcontrol.patch-replace-nonexistent-path-rejected cloudcontrol.update-malformed-patch-document-rejected cloudcontrol.update-resource-invalid-json-patch cloudcontrol.server-enforced-concurrent-operation-limit-rejected cloudcontrol.create-duplicate-identifier-rejected cloudcontrol.create-resource-invalid-type-name cloudcontrol.list-resources-invalid-type-rejected cloudcontrol.list-resources-pagination-stable-ordering cloudcontrol.server-side-handler-failure-propagates-exact-exception-dup-removed
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.base64-blob-property-persists|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.base64-blob-property-round-trip|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native|run#10|AssertionError|assert err["Message"] == expected, (
Issue-Key: cloudcontrol.delete-resource-then-not-found|run#13|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.delete-then-recreate-succeeds|run#10|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.e2e-create-update-observe-delete-flow|run#25|AssertionError|assert _handler_not_found(message, _queue_not_found_detail(identifier)), message
Issue-Key: cloudcontrol.empty-string-vs-absent-property-preserved|run#26|AssertionError|assert _handler_not_found(get_message, _queue_not_found_detail(id_absent)), get_message
Issue-Key: cloudcontrol.get-resource-nonexistent-returns-not-found|run#12|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol|GetResource|ResourceNotFoundException
Issue-Key: cloudcontrol.list-resource-requests-resource-type-filter-preserves-order|run#13|AssertionError|assert _handler_not_found(get_message, _queue_not_found_detail(identifiers[0])), get_message
Issue-Key: cloudcontrol.numeric-precision-property-round-trip|run#24|AssertionError|assert _handler_not_found(get_after_delete_message, _ALARM_NOT_FOUND), get_after_delete_message
Issue-Key: cloudcontrol.progress-event-request-tokens-distinct-and-records-retained|run#22|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.same-identifier-different-types-isolated|run#12|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.timestamp-property-precision-preserved|run#19|AssertionError|assert _sns_topic_not_found(message), message
Issue-Key: cloudcontrol.timestamp-property-preserved|run#17|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.timestamp-property-preserved-exact-format|run#19|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol.concurrent-create-same-identifier-race|run#13|AssertionError|assert message.split(" with RequestToken ", 1)[0] == f"Concurrent operation found for resource {name}", (
Issue-Key: cloudcontrol.concurrent-delete-and-update-same-identifier-race|run#20|AssertionError|assert loser_message in expected_loser, (
Issue-Key: cloudcontrol.concurrent-delete-during-update-race-single-winner|run#21|AssertionError|assert loser_message in expected_loser, (
Issue-Key: cloudcontrol.concurrent-delete-same-identifier-race|run#21|AssertionError|assert message in expected_rejections, (
Issue-Key: cloudcontrol.concurrent-update-same-identifier-race|run#16|AssertionError|assert message.split(" with RequestToken ", 1)[0] == f"Concurrent operation found for resource {name}", (
Issue-Key: cloudcontrol.delete-resource-during-in-progress-create-conflict|run#8|AssertionError|assert message == expected_message, (
Issue-Key: cloudcontrol.update-during-in-progress-create-conflict|run#19|AssertionError|assert conflict_message == expected_conflict, (
Issue-Key: cloudcontrol.client-token-conflict-different-params|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.duplicate-client-token-conflict-rejected|run#18|AssertionError|assert message == (
Issue-Key: cloudcontrol.server-side-unique-request-token-per-operation|run#18|AssertionError|assert conflict_message == expected_conflict, (
Issue-Key: cloudcontrol.server-side-unique-request-token-per-operation-dup-removed|run#17|AssertionError|assert conflict_message == expected_conflict, (
Issue-Key: cloudcontrol.get-resource-request-status-bogus-token|run#16|AssertionError|assert message == f"Request with token {bogus_token} was not found", (
Issue-Key: cloudcontrol.get-resource-request-status-invalid-token-rejected|run#7|AssertionError|assert message == expected_message, (
Issue-Key: cloudcontrol.get-resource-request-status-unknown-token-rejected|run#16|AssertionError|assert message == f"Request with token {bogus_token} was not found", (
Issue-Key: cloudcontrol.request-token-ttl-expiry-status-query|run#7|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: cloudcontrol.create-only-property-mutation-rejected|run#13|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cloudcontrol.patch-multi-op-atomicity-no-partial-apply|run#11|AssertionError|assert diagnostic == expected_diagnostic, (
Issue-Key: cloudcontrol.update-resource-immutable-property-not-updatable|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.update-resource-invalid-client-token-format-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.patch-replace-nonexistent-path-rejected|run#8|AssertionError|assert code == "ValidationException", (
Issue-Key: cloudcontrol.update-malformed-patch-document-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cloudcontrol.update-resource-invalid-json-patch|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cloudcontrol.server-enforced-concurrent-operation-limit-rejected|run#16|AssertionError|assert value == "ConcurrentOperationException", (
Issue-Key: cloudcontrol.create-duplicate-identifier-rejected|run#11|AssertionError|assert code == "AlreadyExists", code
Issue-Key: cloudcontrol.create-resource-invalid-type-name|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudcontrol.list-resources-invalid-type-rejected|_error_code#4|AssertionError|raise AssertionError("expected ListResources against a bogus TypeName to raise, but it succeeded")
Issue-Key: cloudcontrol.list-resources-pagination-stable-ordering|run#22|AssertionError|raise AssertionError("expected ListResources against an unregistered TypeName to be rejected")
Issue-Key: cloudcontrol.server-side-handler-failure-propagates-exact-exception-dup-removed|run#19|AssertionError|assert code_bad_type == "TypeNotFoundException", code_bad_type
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudcontrol: 10 net-new causes — Cloud Control refusal messages, error codes and error classifications diverge from AWS

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
TEN net-new root causes, all in the cloudcontrol (Cloud Control API) service. Each one is an independent defect in how a refusal is worded or classified; a fixer must close all ten before submitting. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json. Note that ValidationException is NOT in that model: it is AWS's front-door input-validation error for this service, so where a cause below says AWS answers ValidationException the basis is the recorded behavior of the check.

Cause 1 of 10: ResourceNotFoundException from a resource-level operation carries a bare internal sentence instead of AWS's handler-wrapped text.
AWS: Smithy models ResourceNotFoundException (HTTP 404, doc "A resource with the specified identifier can't be found") for GetResource/UpdateResource/DeleteResource, and ProgressEvent.ErrorCode targets HandlerErrorCode whose values include NotFound. A not-found is produced by the resource type's handler, so Cloud Control wraps the handler's own diagnostic. The message template is fixed text "<TypeName> Handler returned status FAILED: " + the owning handler's detail + " (HandlerErrorCode: NotFound, RequestToken: <token>)"; the variable parts are the TypeName and Identifier from the request, the handler detail (for SSM: "Resource of type '<TypeName>' with identifier '<Identifier>' was not found."; for SNS: "Topic does not exist (Service: Sns, Status Code: 404, ...)"; for CloudWatch: "Alarm does not exist"), and the server-minted RequestToken.
Emulator: the message is only the inner detail with no wrapper, e.g. "Resource of type 'AWS::SSM::Parameter' with identifier '<id>' was not found." — no "<TypeName> Handler returned status FAILED: " prefix and no "(HandlerErrorCode: NotFound, RequestToken: ...)" suffix. For AWS::SNS::Topic and AWS::CloudWatch::Alarm the handler detail itself is also not the owning service's wording.
Why: clients and IaC tooling parse this message to recover the HandlerErrorCode and the failing request token (it is the only place the handler error code appears on a synchronous refusal). A bare sentence makes NotFound indistinguishable from any other handler failure, and retry/ignore-not-found logic keyed on "HandlerErrorCode: NotFound" never matches.
Why it is not covered elsewhere: this is the message of a correct 404, not the provisioning gap tracked by 20261102T041105Z-cloudcontrol-dynamodb-... .
Fix: the not-found refusal path shared by the GetResource/UpdateResource/DeleteResource handlers in the cloudcontrol crate must compose the handler-wrapped message (type name, per-type handler detail, HandlerErrorCode, request token) rather than returning the raw lookup error.
Failing checks: cloudcontrol.base64-blob-property-persists, cloudcontrol.base64-blob-property-round-trip, cloudcontrol.cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native, cloudcontrol.delete-resource-then-not-found, cloudcontrol.delete-then-recreate-succeeds, cloudcontrol.e2e-create-update-observe-delete-flow, cloudcontrol.empty-string-vs-absent-property-preserved, cloudcontrol.get-resource-nonexistent-returns-not-found, cloudcontrol.list-resource-requests-chronological-order, cloudcontrol.list-resource-requests-resource-type-filter-preserves-order, cloudcontrol.numeric-precision-property-round-trip, cloudcontrol.progress-event-request-tokens-distinct-and-records-retained, cloudcontrol.same-identifier-different-types-isolated, cloudcontrol.timestamp-property-precision-preserved, cloudcontrol.timestamp-property-preserved, cloudcontrol.timestamp-property-preserved-exact-format

Cause 2 of 10: ConcurrentOperationException's Message is the shape's doc sentence instead of AWS's per-resource text.
AWS: Smithy models ConcurrentOperationException (HTTP 409). AWS's message names the resource under contention and, when it has one, the owning request: fixed text "Concurrent operation found for resource " + the resource identifier from the request, optionally followed by " with RequestToken " + the request token that holds the resource.
Emulator: every such refusal carries the literal shape documentation, "Another resource operation is currently being performed on this resource.", naming neither the resource nor the holding token.
Why: a client serializing writes cannot tell WHICH resource or which in-flight request blocked it, so it cannot poll the blocking request to completion and cannot distinguish contention on its own resource from contention reported for another.
Fix: the conflict-detection path shared by the CreateResource/UpdateResource/DeleteResource handlers must build the ConcurrentOperationException message from the contended identifier and the holding request token.
Failing checks: cloudcontrol.concurrent-create-same-identifier-race, cloudcontrol.concurrent-delete-and-update-same-identifier-race, cloudcontrol.concurrent-delete-during-update-race-single-winner, cloudcontrol.concurrent-delete-same-identifier-race, cloudcontrol.concurrent-update-same-identifier-race, cloudcontrol.delete-resource-during-in-progress-create-conflict, cloudcontrol.update-during-in-progress-create-conflict

Cause 3 of 10: ClientTokenConflictException's Message is the shape's doc sentence instead of AWS's per-token text.
AWS: Smithy models ClientTokenConflictException (HTTP 409) for CreateResource/UpdateResource/DeleteResource. AWS's message is fixed text "ClientToken " + the ClientToken from the request + " is already associated with an existing operation with RequestToken " + the RequestToken of the request that first used it.
Emulator: "The specified client token has already been used in another resource request." — the shape's doc text, naming neither the token nor the owning request.
Why: the whole point of the conflict is to let the caller find the earlier operation; without the RequestToken a client that retried with a reused token cannot poll the original request and cannot tell which of its tokens collided.
Fix: the client-token bookkeeping behind the three mutating handlers must carry the first-use RequestToken and render it (with the submitted ClientToken) into the conflict message.
Failing checks: cloudcontrol.client-token-conflict-different-params, cloudcontrol.duplicate-client-token-conflict-rejected, cloudcontrol.server-side-unique-request-token-per-operation, cloudcontrol.server-side-unique-request-token-per-operation-dup-removed

Cause 4 of 10: RequestTokenNotFoundException's Message is the shape's doc sentence instead of AWS's per-token text.
AWS: Smithy models RequestTokenNotFoundException (HTTP 404) as the only error of GetResourceRequestStatus. AWS's message is fixed text "Request with token " + the RequestToken from the request + " was not found" (no trailing period).
Emulator: "A resource operation with the specified request token can't be found." — the doc sentence, never naming the token that was not found.
Why: a client polling several requests (and one whose 36-hour record has expired) cannot attribute the 404 to a particular token from the message, which is all it gets back.
Fix: the GetResourceRequestStatus handler's unknown/expired-token path must render the submitted token into the message.
Failing checks: cloudcontrol.get-resource-request-status-bogus-token, cloudcontrol.get-resource-request-status-invalid-token-rejected, cloudcontrol.get-resource-request-status-unknown-token-rejected, cloudcontrol.request-token-ttl-expiry-status-query

Cause 5 of 10: the create-only refusal message is an ad-hoc sentence instead of AWS's patch diagnostic.
AWS: Smithy models NotUpdatableException (HTTP 400, doc "One or more properties included in this resource operation are defined as create-only, and therefore can't be updated"). AWS's message is fixed text "Invalid patch update: createOnlyProperties [" + the comma-separated JSON pointers of the offending create-only properties, as the type's registry schema declares them (e.g. /properties/QueueName, /properties/Name) + "] cannot be updated".
Emulator: "Cannot update create-only property '<PropertyName>'." — different fixed text, and it names a bare property name rather than the schema's /properties/<Name> pointer.
Why: the pointer form is what identifies the property in the resource's registry schema; tooling that maps the refusal back to the schema (or lists several offending properties at once) cannot parse the emulator's sentence, and a multi-property violation cannot be reported at all.
Fix: the create-only rejection in the UpdateResource validation path must emit the createOnlyProperties pointer-list diagnostic.
Failing checks: cloudcontrol.create-only-property-mutation-rejected, cloudcontrol.patch-multi-op-atomicity-no-partial-apply, cloudcontrol.update-resource-immutable-property-not-updatable

Cause 6 of 10: the clientToken constraint-violation message echoes the anchored regex.
AWS: the ClientToken member is modeled with pattern ^[-A-Za-z0-9+/=]+$ (length 1-128), but AWS's front-door ValidationException renders the constraint unanchored: fixed text "1 validation error detected: Value '" + the submitted value + "' at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9+/=]+".
Emulator: identical except that it prints the anchored form "^[-A-Za-z0-9+/=]+$", i.e. it echoes the Smithy pattern trait verbatim.
Why: the message is the contract for input-validation failures; clients and test suites match it exactly, and emitting the anchors makes every clientToken (and any other pattern-constrained member rendered the same way) validation message a mismatch.
Fix: the request-validation message formatter must strip the leading ^ and trailing $ when rendering a pattern trait into a validation message.
Failing check: cloudcontrol.update-resource-invalid-client-token-format-rejected

Cause 7 of 10: patch-document failures are classified InvalidRequestException instead of ValidationException.
AWS: recorded behavior only (ValidationException is not in the on-disk model; InvalidRequestException is, HTTP 400, doc "The resource handler has returned that invalid input from the user has generated a generic exception"). A PatchDocument that is not a well-formed JSON Patch, or whose op targets a path the resource model does not have, is rejected by Cloud Control's front door with error code ValidationException before any handler runs; InvalidRequestException is reserved for input the HANDLER rejects.
Emulator: UpdateResource answers InvalidRequestException with "Invalid request provided: the patch document could not be applied to the resource model" for both a malformed patch and a replace against an absent path.
Why: the two codes mean different things to a client — ValidationException is a non-retryable caller bug in the request, InvalidRequestException reports a downstream handler verdict — so error handling and retry classification branch the wrong way, and the "replace of an absent path" case is indistinguishable from a handler rejection.
Fix: the PatchDocument parse/apply step in the UpdateResource handler must raise the front-door ValidationException for a malformed patch or an unresolvable path, keeping InvalidRequestException for handler-reported input errors.
Failing checks: cloudcontrol.patch-replace-nonexistent-path-rejected, cloudcontrol.update-malformed-patch-document-rejected, cloudcontrol.update-resource-invalid-json-patch

Cause 8 of 10: an UpdateResource overlapping an in-flight operation is refused with the wrong error code.
AWS: Smithy models ConcurrentOperationException (HTTP 409) on UpdateResource for exactly this case — a second mutating request while one is in flight on the same resource is refused with ConcurrentOperationException (message per Cause 2).
Emulator: the overlapping UpdateResource is refused with InvalidRequestException.
Why: ConcurrentOperationException is the signal to back off and retry once the in-flight request terminates; InvalidRequestException reads as a permanent bad request, so a client gives up on an operation that would have succeeded moments later.
Fix: the concurrency guard in the UpdateResource handler must classify an overlap as ConcurrentOperationException rather than falling through to the generic invalid-request path.
Failing check: cloudcontrol.server-enforced-concurrent-operation-limit-rejected

Cause 9 of 10: a duplicate CreateResource reports Cloud Control's own AlreadyExistsException instead of the owning handler's AlreadyExists verdict.
AWS: ProgressEvent.ErrorCode targets HandlerErrorCode, whose values include AlreadyExists; a create whose resource already exists fails with the handler's error code "AlreadyExists" and the owning service's message — for AWS::SQS::Queue the fixed text "Queue creation failed because the queue already exists" (no variable part).
Emulator: the refusal carries code "AlreadyExistsException" (the Cloud Control exception shape name) and a different message.
Why: HandlerErrorCode values and Cloud Control exception names are separate vocabularies; a client switching on the handler error code "AlreadyExists" (the documented contract for "the resource is already there, treat as idempotent") never matches "AlreadyExistsException" and treats an existing resource as a hard failure.
Fix: the duplicate-identifier path in the CreateResource handler must surface the owning handler's error code and message (HandlerErrorCode AlreadyExists plus the service's own text) instead of substituting the Cloud Control shape name.
Failing check: cloudcontrol.create-duplicate-identifier-rejected

Cause 10 of 10: a TypeName that is not in the CloudFormation registry is not refused with TypeNotFoundException.
AWS: Smithy models TypeNotFoundException (HTTP 404, doc "The specified extension doesn't exist in the CloudFormation registry") on CreateResource, GetResource and ListResources. A syntactically valid but unregistered TypeName (e.g. AWS::NoSuch::Service...) is refused with TypeNotFoundException; only a registered type proceeds.
Emulator: CreateResource with an unregistered TypeName is ACCEPTED and answers 200 with a ProgressEvent in IN_PROGRESS; ListResources against a bogus/unregistered TypeName returns a successful (empty) page; and GetResource of an unregistered type answers ResourceNotFoundException instead of TypeNotFoundException, conflating "no such type" with "no such resource".
Why: a typo in a TypeName silently becomes a request that appears to succeed (or an empty inventory), so IaC and drift tooling reports nothing wrong while provisioning nothing; and a client cannot distinguish an unregistered type (fix the template) from a missing resource (create it).
Fix: a registry lookup of TypeName at the front door of the CreateResource/GetResource/ListResources handlers must refuse unregistered types with TypeNotFoundException before any resource lookup or request record is created.
Failing checks: cloudcontrol.create-resource-invalid-type-name, cloudcontrol.list-resources-invalid-type-rejected, cloudcontrol.list-resources-pagination-stable-ordering, cloudcontrol.server-side-handler-failure-propagates-exact-exception-dup-removed

Traces: the failing check ids are listed per cause above and collected in Issue-Trace-Ids. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs, update_resource.rs, delete_resource.rs, get_resource.rs, get_resource_request_status.rs, list_resources.rs and the shared store in src/state.rs.

Related open issues that do NOT cover any cause here: 20261102T041105Z-cloudcontrol-dynamodb-... and 20261102T044041Z-cloudcontrol-sqs-... concern provisioning/read-through, not refusal wording or classification; 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent... concerns the Identifier of a SUCCESS event.

## Evidence
All quotes are from the replayed checks (suite simparity-behavioral-aws-v1). Per-run values (generated names, uuids, hosts, request tokens) fill the templates; they are not part of the requirement.

Cause 1 — expected "AWS::SSM::Parameter Handler returned status FAILED: Resource of type 'AWS::SSM::Parameter' with identifier '<id>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)"; got "Resource of type 'AWS::SSM::Parameter' with identifier '<id>' was not found." (ResourceNotFoundException, 404). For AWS::SNS::Topic the expected prefix is "AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, " with the same "(HandlerErrorCode: NotFound, RequestToken: ...)" suffix; for AWS::CloudWatch::Alarm "AWS::CloudWatch::Alarm Handler returned status FAILED: Alarm does not exist". cloudcontrol.cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native fails as: "the refusal message does not name the handler error code: \"Resource of type 'AWS::SSM::Parameter' with identifier '<id>' was not found.\"".

Cause 2 — "expected the loser's message to be one of ('Concurrent operation found for resource <name>', 'Concurrent operation found for resource <name> with RequestToken <token>'), got 'Another resource operation is currently being performed on this resource.'" (ConcurrentOperationException, 409).

Cause 3 — "expected ClientTokenConflictException message 'ClientToken <token> is already associated with an existing operation with RequestToken <request-token>', got 'The specified client token has already been used in another resource request.'" (409).

Cause 4 — "expected the refusal message 'Request with token <token> was not found', got \"A resource operation with the specified request token can't be found.\"" (RequestTokenNotFoundException, 404).

Cause 5 — "expected the message 'Invalid patch update: createOnlyProperties [/properties/QueueName] cannot be updated', got \"Cannot update create-only property 'QueueName'.\"" and the same for /properties/Name (NotUpdatableException, 400).

Cause 6 — expected "1 validation error detected: Value 'not a valid token!!' at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9+/=]+", got the identical message ending "...pattern: ^[-A-Za-z0-9+/=]+$".

Cause 7 — "replace against an absent path must be refused with ValidationException, got InvalidRequestException: Invalid request provided: the patch document could not be applied to the resource model"; also "expected ValidationException, got InvalidRequestException" for a malformed patch document.

Cause 8 — "expected a rejected overlapping UpdateResource to fail with ConcurrentOperationException, got 'InvalidRequestException'".

Cause 9 — expected error code "AlreadyExists" with message "Queue creation failed because the queue already exists"; got "AlreadyExistsException".

Cause 10 — CreateResource(TypeName="AWS::NoSuch::Service...", Identifier="whatever") returned HTTP 200 with ProgressEvent OperationStatus="IN_PROGRESS" where TypeNotFoundException (404) is required; "expected ListResources against a bogus TypeName to raise, but it succeeded"; "expected ListResources against an unregistered TypeName to be rejected"; and "assert code_bad_type == 'TypeNotFoundException'" got "ResourceNotFoundException".

## Steps to Reproduce
Boot the emulator and point botocore at it; cc = client("cloudcontrol").

Cause 1: create any resource through cc (e.g. TypeName="AWS::SSM::Parameter", DesiredState={"Name": n, "Type": "String", "Value": "v"}), poll get_resource_request_status to SUCCESS, delete_resource and poll to SUCCESS, then cc.get_resource(TypeName=..., Identifier=...) — inspect Error.Message: it must be "<TypeName> Handler returned status FAILED: <handler detail> (HandlerErrorCode: NotFound, RequestToken: <token>)".
Cause 2: submit two mutating requests on the same identifier so the second overlaps the first; inspect the ConcurrentOperationException Message — it must name the identifier and the holding RequestToken.
Cause 3: cc.create_resource(..., ClientToken=t) twice with different DesiredState; the 409 Message must be "ClientToken t is already associated with an existing operation with RequestToken <first token>".
Cause 4: cc.get_resource_request_status(RequestToken="<a well-formed token nothing minted>") — Message must be "Request with token <that token> was not found".
Cause 5: cc.update_resource on an AWS::SQS::Queue with PatchDocument [{"op":"replace","path":"/QueueName","value":"other"}] — Message must be "Invalid patch update: createOnlyProperties [/properties/QueueName] cannot be updated".
Cause 6: cc.update_resource(..., ClientToken="not a valid token!!") — the validation Message must end "pattern: [-A-Za-z0-9+/=]+".
Cause 7: cc.update_resource(..., PatchDocument="not json") and PatchDocument [{"op":"replace","path":"/NoSuchProp","value":1}] — both must be ValidationException, not InvalidRequestException.
Cause 8: issue a second UpdateResource while the first is still in flight on the same Identifier — must be ConcurrentOperationException.
Cause 9: create an AWS::SQS::Queue through cc, wait for SUCCESS, then create_resource the same QueueName with a different VisibilityTimeout — the failure must carry error code "AlreadyExists" and message "Queue creation failed because the queue already exists".
Cause 10: cc.create_resource(TypeName="AWS::NoSuch::Servicexyz", DesiredState="{}") , cc.list_resources(TypeName="AWS::NoSuch::Servicexyz") and cc.get_resource(TypeName="AWS::NoSuch::Servicexyz", Identifier="x") — all three must be TypeNotFoundException (404).
