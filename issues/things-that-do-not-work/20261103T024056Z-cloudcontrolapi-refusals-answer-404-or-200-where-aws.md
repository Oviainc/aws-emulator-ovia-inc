Issue-Source: trace-sweep-divergence
Issue-Trace: cloudcontrol-create-resource-unknown-type
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudcontrol-create-resource-unknown-type cloudcontrol-list-requests-empty-filters-and-type-error cloudcontrolapi-unregistered-type-rejected cloudcontrol-get-request-status-unknown-token cloudcontrol-invalid-request-token-error-paths cloudcontrolapi-unknown-request-token-handling cloudcontrolapi-unknown-identifier-operations cloudcontrol-list-requests-invalid-operation-filter
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrolapi|CreateResource|4xx->2xx|$.Message
Issue-Key: cloudcontrolapi|CreateResource|4xx->2xx|$.__type
Issue-Key: cloudcontrolapi|CreateResource|4xx->2xx|status
Issue-Key: cloudcontrolapi|ListResources|4xx->2xx|$.Message
Issue-Key: cloudcontrolapi|ListResources|4xx->2xx|$.__type
Issue-Key: cloudcontrolapi|ListResources|4xx->2xx|status
Issue-Key: cloudcontrolapi|GetResourceRequestStatus|4xx->4xx|$.Message
Issue-Key: cloudcontrolapi|GetResourceRequestStatus|4xx->4xx|status
Issue-Key: cloudcontrolapi|GetResource|4xx->4xx|$.Message
Issue-Key: cloudcontrolapi|GetResource|4xx->4xx|status
Issue-Key: cloudcontrolapi|UpdateResource|4xx->4xx|$.Message
Issue-Key: cloudcontrolapi|UpdateResource|4xx->4xx|status
Issue-Key: cloudcontrolapi|ListResourceRequests|4xx->4xx|$.message
Issue-Commit: 74eadbff5baefcf7d85baaabfcce8fef6d457a2e

# cloudcontrolapi refusals answer 404 or 200 where AWS answers 400 with the modeled error payload

> _Note: this issue was identified against the code base at commit `74eadbff5baefcf7d85baaabfcce8fef6d457a2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
SIX net-new root causes in the cloudcontrolapi (Cloud Control API, sdkId CloudControl, awsJson1_0) service, all in how a REFUSAL is produced: the emulator either does not refuse at all, or refuses with an HTTP status and a Message that are not the ones Cloud Control puts on the wire. A fixer must close all six before submitting. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Protocol note that applies to causes 1-5: the Smithy model annotates TypeNotFoundException, ResourceNotFoundException and RequestTokenNotFoundException with @httpError 404, but the RECORDED wire responses for this awsJson1_0 service carry HTTP 400 for all three. The recording is the wire truth here, so 400 is the requirement and the model's httpError must not be used as the response status for these shapes.

Cause 1 of 6: CreateResource ACCEPTS a TypeName that is not in the CloudFormation registry.
AWS: Smithy models TypeNotFoundException on CreateResource (doc: "The specified extension doesn't exist in the CloudFormation registry"); recorded wire response is HTTP 400 with __type "com.amazon.cloudapiservice#TypeNotFoundException" and Message whose template is the fixed text "The type '" + the TypeName from the request + "' cannot be found." A registry lookup happens at the front door, before any request record is minted.
Emulator: answers HTTP 200 with a ProgressEvent and no error at all — __type and Message are absent.
Why: a typo'd or unsupported TypeName looks like a successful provisioning request, so IaC and drift tooling records a request token for a resource that will never exist and never learns the type is unregistered.
Fix: a TypeName registry lookup at the front of the CreateResource handler (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs) must refuse unregistered types with TypeNotFoundException at HTTP 400 before any state mutation.
Traces: cloudcontrol-create-resource-unknown-type#task#1 (cluster also covers cloudcontrol-list-requests-empty-filters-and-type-error#task#1, cloudcontrolapi-unregistered-type-rejected#task#1).

Cause 2 of 6: ListResources ACCEPTS an unregistered TypeName and returns a successful page.
AWS: Smithy models TypeNotFoundException on ListResources too; recorded response is HTTP 400, __type "com.amazon.cloudapiservice#TypeNotFoundException", Message "The type '" + the request's TypeName + "' cannot be found."
Emulator: HTTP 200 with no __type and no Message (an empty/successful listing).
Why: an inventory caller cannot distinguish "this type is not registered" from "this account owns none of that type", so discovery silently reports an empty inventory for a type it should have rejected.
Fix: the same front-door registry lookup in the ListResources handler (src/ops/list_resources.rs), before the store is consulted or a page is assembled.
Traces: cloudcontrolapi-unregistered-type-rejected#task#2.

Cause 3 of 6: GetResourceRequestStatus for an unknown request token answers the wrong status and the shape's doc sentence.
AWS: Smithy models RequestTokenNotFoundException as the only error of GetResourceRequestStatus; recorded wire status is HTTP 400 and the Message template is the fixed text "Request with token " + the RequestToken from the request + " was not found" (no trailing period).
Emulator: HTTP 404 with Message "A resource operation with the specified request token can't be found." — that is the shape's documentation text, and it never names the token.
Why: the Message is all a poller gets back; a client tracking several tokens (or one whose record has aged out) cannot attribute the refusal to a particular token, and a 404 vs 400 difference re-routes status-code-based retry classification.
Fix: the unknown/expired-token path of the GetResourceRequestStatus handler (src/ops/get_resource_request_status.rs) must render the submitted token into the message and emit the refusal at HTTP 400.
Traces: cloudcontrol-get-request-status-unknown-token#task#1 (cluster also covers cloudcontrol-invalid-request-token-error-paths#task#1, cloudcontrolapi-unknown-request-token-handling#task#2).

Cause 4 of 6: GetResource for an absent identifier answers HTTP 404 and a bare lookup sentence instead of HTTP 400 with the handler-wrapped diagnostic.
AWS: ResourceNotFoundException is modeled on GetResource; the recorded response is HTTP 400 and its Message is the handler-wrapped template: the fixed text "<TypeName> Handler returned status FAILED: " + the owning handler's own detail + " (HandlerErrorCode: NotFound, RequestToken: <token>)". ProgressEvent.ErrorCode targets HandlerErrorCode, whose values include NotFound, which is why the handler verdict is carried inside the message. The variable parts are the TypeName from the request, the Identifier as the handler renders it (for AWS::Logs::LogGroup the primary-identifier JSON form, i.e. a {"/properties/<PrimaryIdProperty>":"<value>"} object rather than the bare string the caller passed), and the server-minted RequestToken.
Emulator: HTTP 404 with only the inner detail — "Resource of type '<TypeName>' with identifier '<identifier as passed>' was not found." — no "Handler returned status FAILED" prefix, no "(HandlerErrorCode: NotFound, RequestToken: ...)" suffix, and the identifier rendered as the bare string.
Why: the message is the only place a synchronous refusal exposes the HandlerErrorCode, so ignore-if-not-found logic keyed on "HandlerErrorCode: NotFound" never matches, and the wrong status misclassifies the refusal.
Fix: the shared not-found refusal path behind GetResource (src/ops/get_resource.rs, which calls crate::state::resource_not_found_message in src/state.rs) must compose the handler-wrapped message and answer HTTP 400.
Why it is not already covered: open issue 20261102T095426Z-cloudcontrol-10-net-new-causes-cloud-control-refusal.md Cause 1 describes the missing wrapper for behavioral checks, but neither it nor any other open issue covers the HTTP status (400 vs 404) or the JSON primary-identifier rendering this wire fixture pins; the replay reports no owning issue for this step.
Traces: cloudcontrolapi-unknown-identifier-operations#task#1.

Cause 5 of 6: UpdateResource for an absent identifier has the same defect, on its own code path.
AWS: as Cause 4 — recorded HTTP 400 with the "<TypeName> Handler returned status FAILED: ... (HandlerErrorCode: NotFound, RequestToken: <token>)" message, with the identifier in its JSON primary-identifier form.
Emulator: HTTP 404 with the bare "Resource of type '<TypeName>' with identifier '<identifier>' was not found." sentence.
Why: an update against a resource that was deleted out from under the caller is indistinguishable from any other handler failure, and the status is wrong.
Fix: the not-found branch of the UpdateResource handler (src/ops/update_resource.rs) must use the same wrapped-message/400 path as Cause 4.
Traces: cloudcontrolapi-unknown-identifier-operations#task#2.

Cause 6 of 6: the ListResourceRequests enum validation message is rendered per list MEMBER instead of per list, and with the wrong enum ordering.
AWS: Smithy models ResourceRequestStatusFilter.Operations as a list of Operation (values CREATE, DELETE, UPDATE). The recorded front-door ValidationException message is the fixed template "1 validation error detected: Value '" + the whole submitted list in bracketed form + "' at 'resourceRequestStatusFilter.operations' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [" + the allowed values + "]]" — the path names the LIST member, the offending value is printed as the bracketed list, and the enum set is wrapped in the nested "Member must satisfy constraint: [...]" clause. Recorded behavior also prints the value set as "UPDATE, DELETE, CREATE" rather than the model's declaration order.
Emulator: HTTP 400 ValidationException, but the message is "1 validation error detected: Value '<bare value>' at 'resourceRequestStatusFilter.operations.1.member' failed to satisfy constraint: Member must satisfy enum value set: [CREATE, DELETE, UPDATE]" — an indexed ".<n>.member" path, an unbracketed value, no nested constraint clause, and a different value ordering.
Why: validation messages are the documented contract for malformed input; clients and conformance suites match them exactly, so every list-of-enum rejection on this service is a mismatch.
Fix: the request-validation message formatter used by the ListResourceRequests handler (src/ops/list_resource_requests.rs and the shared validation renderer) must render a constrained list member at the list path with the bracketed value and the nested enum-set clause.
Why it is not already covered: open issue 20261102T095426Z-cloudcontrol-listresourcerequests-stub.md tracks the operation being an unimplemented scaffold; this step is NOT blocked (the emulator answered 400 with a message), and the replay reports no stub_path and no owning issue for it, so the message rendering is a separate net-new defect.
Traces: cloudcontrol-list-requests-invalid-operation-filter#task#1.

## Evidence
All quotes are from the replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false). Generated names, uuids and request tokens fill the templates and are not part of the requirement.

Cause 1 — fixture cloudcontrol-create-resource-unknown-type, step task#1, CreateResource: "status: expected 400, got 200; $.__type: missing (expected 'com.amazon.cloudapiservice#TypeNotFoundException', got None); $.Message: missing (expected \"The type 'AWS::NoSuchService::FakeResource' cannot be found.\", got None)". Fixed text: "The type '" ... "' cannot be found."; variable part: the TypeName from the request. 3 affected steps.

Cause 2 — fixture cloudcontrolapi-unregistered-type-rejected, step task#2, ListResources: "status: expected 400, got 200; $.__type: missing (expected 'com.amazon.cloudapiservice#TypeNotFoundException', got None); $.Message: missing (expected \"The type 'AWS::SimParityProbe::NoSuchType' cannot be found.\", got None)".

Cause 3 — fixture cloudcontrol-get-request-status-unknown-token, step task#1, GetResourceRequestStatus: "status: expected 400, got 404; $.Message: value_mismatch (expected 'Request with token cc-never-issued-...-dec6cdf2 was not found', got \"A resource operation with the specified request token can't be found.\")". Fixed text: "Request with token " ... " was not found"; variable part: the RequestToken from the request. 3 affected steps.

Cause 4 — fixture cloudcontrolapi-unknown-identifier-operations, step task#1, GetResource: "status: expected 400, got 404; $.Message: value_mismatch (expected 'AWS::Logs::LogGroup Handler returned status FAILED: Resource of type \\'AWS::Logs::LogGroup\\' with identifier \\'{\"/properties/LogGroupName\":\"<name>\"}\\' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)', got \"Resource of type 'AWS::Logs::LogGroup' with identifier '<name>' was not found.\")". Fixed text: " Handler returned status FAILED: ", "Resource of type '...' with identifier '...' was not found.", "(HandlerErrorCode: NotFound, RequestToken: "; variable parts: TypeName and Identifier from the request (identifier in its /properties JSON form) and the server-minted token.

Cause 5 — same fixture, step task#2, UpdateResource: identical expected/actual pair with its own RequestToken.

Cause 6 — fixture cloudcontrol-list-requests-invalid-operation-filter, step task#1, ListResourceRequests, both sides HTTP 400: "$.message: value_mismatch (expected \"1 validation error detected: Value '[NOT_A_REAL_OPERATION]' at 'resourceRequestStatusFilter.operations' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [UPDATE, DELETE, CREATE]]\", got \"1 validation error detected: Value 'NOT_A_REAL_OPERATION' at 'resourceRequestStatusFilter.operations.1.member' failed to satisfy constraint: Member must satisfy enum value set: [CREATE, DELETE, UPDATE]\")".

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1, or drive it with botocore: cc = client("cloudcontrol", endpoint_url=<emulator>).

Cause 1: cc.create_resource(TypeName="AWS::NoSuchService::FakeResource", DesiredState="{}") — must be HTTP 400 TypeNotFoundException with Message "The type 'AWS::NoSuchService::FakeResource' cannot be found."; the emulator answers 200 with a ProgressEvent.
Cause 2: cc.list_resources(TypeName="AWS::SimParityProbe::NoSuchType") — must be HTTP 400 TypeNotFoundException with the same message template; the emulator answers 200.
Cause 3: cc.get_resource_request_status(RequestToken="<well-formed token nothing minted>") — must be HTTP 400 RequestTokenNotFoundException with Message "Request with token <that token> was not found".
Cause 4: cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier="<a log group that does not exist>") — must be HTTP 400 ResourceNotFoundException whose Message is "AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '{\"/properties/LogGroupName\":\"<name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)".
Cause 5: cc.update_resource(TypeName="AWS::Logs::LogGroup", Identifier="<same absent name>", PatchDocument="[]") — same status and message template.
Cause 6: cc.list_resource_requests(ResourceRequestStatusFilter={"Operations": ["NOT_A_REAL_OPERATION"]}) — the 400 message must name the list path 'resourceRequestStatusFilter.operations', print the value bracketed, and wrap the enum set in "Member must satisfy constraint: [...]".
