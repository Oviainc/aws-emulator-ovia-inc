Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.webapp-lifecycle-create-describe-delete transfer.delete-webapp-customization-while-referenced-conflict
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.webapp-lifecycle-create-describe-delete|run#15|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Key: transfer.delete-webapp-customization-while-referenced-conflict|run#19|AssertionError|assert codes <= {None, "ConflictException"}, (
Issue-Commit: 8a374ab335c3a9ff827299573023e94ba7131cd9

# Transfer web-app plane incomplete: UpdateWebApp and DeleteWebAppCustomization answer 501 NotImplemented instead of their modeled success/404/409 behaviour (2 causes)

> _Note: this issue was identified against the code base at commit `8a374ab335c3a9ff827299573023e94ba7131cd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebApp

## Description
Two net-new root causes in the Transfer Family web-app control plane. Both are operations whose handler is still the generated scaffold: the handler body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`, so every call — well-formed or not, on a live or a deleted resource — is answered HTTP 501 with error code `NotImplemented` and message `operation <Op> is scaffolded but not implemented in this emulator`. Neither operation is named by any open issue (the only Transfer stub issue filed is 20261022T110916Z-transfer-importcertificate-stub.md, for ImportCertificate; the WebApp failures are also not covered by 20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md or 20261022T202037Z-transfer-host-key-plane-broken-importhostkey-refuses-valid.md, which own other WebApp checks for other reasons). Note that the surrounding WebApp operations (CreateWebApp, DescribeWebApp, DeleteWebApp, UpdateWebAppCustomization) ARE implemented and answer 200, so the plane is half-present: a client can create and delete a web app but can neither update one nor delete its customization.

Cause 1 of 2: UpdateWebApp is a 501 stub, so it never reaches its success path and never produces ResourceNotFoundException for a dead WebAppId.
AWS: Smithy UpdateWebApp (POST /updateWebApp, awsJson, SUCCESS_STATUS 200) takes `WebAppId` (@required, WebAppId @length 24..24, @pattern ^webapp-[0-9a-f]{17}$) plus optional IdentityProviderDetails / AccessEndpoint (@length 1..1024) / WebAppUnits / EndpointDetails, and returns `WebAppId` (@required) on success. Its modeled errors are AccessDeniedException (403), ConflictException (409), InternalServiceError (503), InvalidRequestException (400), ResourceNotFoundException (404) — "This exception is thrown when a resource is not found by the Amazon Web Services Transfer Family service" — and ThrottlingException (429). 501/NotImplemented is not among them. Recorded behavior: after DeleteWebApp, a subsequent UpdateWebApp on the same id is refused with HTTP 404 `ResourceNotFoundException` and message `Unknown WebApp`; the whole message is fixed text with no variable part (the WebAppId is NOT interpolated), the same message DescribeWebApp returns for the deleted id.
Emulator: the call returns HTTP 501 `NotImplemented` ("operation UpdateWebApp is scaffolded but not implemented in this emulator") for every input, including a valid mutation of a live web app and a mutation of a deleted WebAppId.
Why: a client cannot change a web app's units, access endpoint, endpoint details or identity-provider configuration at all, and its error handling breaks twice over — a retryable-looking server-class 501 is returned where AWS returns a terminal client-class 404, so SDK retry logic retries a call that will never succeed, and code that distinguishes "gone" (ResourceNotFoundException) from "transient" cannot see that the web app was deleted.
Fix: the operation's handler, services/service-com-amazonaws-transfer-transferservice/src/ops/update_web_app.rs — replace the `not_implemented` return in `UpdateWebApp::handle` with real behaviour against the web-app store (apply the supplied members to the stored web app and return its WebAppId; refuse an unknown or already-deleted WebAppId with ResourceNotFoundException/404, message `Unknown WebApp`) and delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line.
Traces: transfer.webapp-lifecycle-create-describe-delete — `AssertionError: expected UpdateWebApp on a deleted WebAppId to also fail with ResourceNotFoundException, got 'NotImplemented'`.

Cause 2 of 2: DeleteWebAppCustomization is a 501 stub, so a web app's customization can be created and read but never removed.
AWS: Smithy DeleteWebAppCustomization (POST /deleteWebAppCustomization, awsJson, @idempotent, SUCCESS_STATUS 200) takes only `WebAppId` (@required, @length 24..24, @pattern ^webapp-[0-9a-f]{17}$) and has an empty output (output shape Unit). Its modeled errors are AccessDeniedException (403), ConflictException (409), InternalServiceError (503), InvalidRequestException (400), ResourceNotFoundException (404) and ThrottlingException (429) — again, 501/NotImplemented is not modeled. Recorded behavior: when it races an UpdateWebAppCustomization on the same WebAppId, each racer either succeeds (200, empty body) or fails with ConflictException (409) and nothing else, and at least one racer lands; after the delete wins, DescribeWebAppCustomization answers HTTP 404 `ResourceNotFoundException` with message `The WebApp <WebAppId> doesn't have customization` — fixed text `The WebApp ` + the request's WebAppId + ` doesn't have customization`, i.e. the only variable part is the WebAppId taken from the request.
Emulator: the call returns HTTP 501 `NotImplemented` ("operation DeleteWebAppCustomization is scaffolded but not implemented in this emulator"), while the racing UpdateWebAppCustomization succeeds — so the observed outcome set is {update: success, delete: NotImplemented} where AWS admits only success or ConflictException.
Why: the customization is write-only and immortal — a client that has set a Title/LogoFile/FaviconFile on a web app can never clear it, and the 501 is a server-class status the SDK may retry even though the operation is @idempotent and would be safe to retry only if it were really implemented. A caller cannot ever observe the documented "no customization" 404 state.
Fix: the operation's handler, services/service-com-amazonaws-transfer-transferservice/src/ops/delete_web_app_customization.rs — replace the `not_implemented` return in `DeleteWebAppCustomization::handle` with real behaviour against the customization store (remove the customization for the given WebAppId and answer 200 with an empty body; refuse an unknown WebAppId with ResourceNotFoundException/404) and delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line; DescribeWebAppCustomization must then report the deleted state as 404 `ResourceNotFoundException` with the message above.
Traces: transfer.delete-webapp-customization-while-referenced-conflict — `AssertionError: unexpected error code(s) among racers, expected only success or ConflictException, got outcomes={'update': None, 'delete': 'NotImplemented'}`.

## Evidence
Cause 1 — transfer.webapp-lifecycle-create-describe-delete (mismatch):
  check_transfer_webapp_lifecycle_create_describe_delete.py line 53
  AssertionError: expected UpdateWebApp on a deleted WebAppId to also fail with ResourceNotFoundException, got 'NotImplemented'
  Expected: HTTP 404, Error.Code='ResourceNotFoundException', Error.Message='Unknown WebApp' (fixed text, no interpolation).
  Actual:   HTTP 501, Error.Code='NotImplemented', Error.Message='operation UpdateWebApp is scaffolded but not implemented in this emulator'.
  Earlier beats of the same check pass: CreateWebApp -> 200 with a WebAppId, DescribeWebApp -> 200 echoing it, DeleteWebApp -> 200, DescribeWebApp on the dead id -> 404 ResourceNotFoundException 'Unknown WebApp'. Only the UpdateWebApp beat diverges, which isolates the cause to that handler.
  Source: services/service-com-amazonaws-transfer-transferservice/src/ops/update_web_app.rs line 4 ('//! SCAFFOLDED, NOT IMPLEMENTED:') and line 178 ('Err(crate::scaffolded::Scaffolded::not_implemented("UpdateWebApp"))'), while lines 104-113 show the model's six real errors (403/409/503/400/404/429) that the handler can already construct.

Cause 2 — transfer.delete-webapp-customization-while-referenced-conflict (mismatch):
  check_transfer_delete_webapp_customization_while_referenced_conflict.py line 80
  AssertionError: unexpected error code(s) among racers, expected only success or ConflictException, got outcomes={'update': None, 'delete': 'NotImplemented'}
  Expected: each racer in {200 success, HTTP 409 ConflictException}; and if the delete won, DescribeWebAppCustomization -> HTTP 404 ResourceNotFoundException with Message='The WebApp <WebAppId> doesn\'t have customization'.
  Actual:   UpdateWebAppCustomization -> 200; DeleteWebAppCustomization -> HTTP 501, Error.Code='NotImplemented', Error.Message='operation DeleteWebAppCustomization is scaffolded but not implemented in this emulator'.
  The setup beats pass: CreateWebApp -> 200, UpdateWebAppCustomization(Title='original-title') -> 200. Only the delete leg diverges.
  Source: services/service-com-amazonaws-transfer-transferservice/src/ops/delete_web_app_customization.rs line 4 ('//! SCAFFOLDED, NOT IMPLEMENTED:').
  Smithy (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DeleteWebAppCustomization input {WebAppId required, length 24..24, pattern ^webapp-[0-9a-f]{17}$}, output Unit, @idempotent, errors AccessDeniedException/403, ConflictException/409, InternalServiceError/503, InvalidRequestException/400, ResourceNotFoundException/404, ThrottlingException/429 — NotImplemented/501 is not modeled for either operation.

## Steps to Reproduce
Cause 1 (UpdateWebApp):
1. sso-admin ListInstances (or CreateInstance) to obtain an IAM Identity Center InstanceArn; iam CreateRole for a transfer.amazonaws.com trust policy.
2. transfer CreateWebApp(IdentityProviderDetails={IdentityCenterConfig:{InstanceArn, Role}}) -> note WebAppId.
3. transfer UpdateWebApp(WebAppId=<that id>, WebAppUnits={Provisioned: 1}) -> emulator answers HTTP 501 NotImplemented; AWS answers 200 with {WebAppId}.
4. transfer DeleteWebApp(WebAppId=<that id>), then transfer UpdateWebApp(WebAppId=<that id>, WebAppUnits={Provisioned: 1}) -> emulator answers HTTP 501 NotImplemented; AWS answers HTTP 404 ResourceNotFoundException with message 'Unknown WebApp'.

Cause 2 (DeleteWebAppCustomization):
1. Same prerequisites; transfer CreateWebApp -> WebAppId.
2. transfer UpdateWebAppCustomization(WebAppId, Title='original-title') -> 200.
3. transfer DeleteWebAppCustomization(WebAppId) -> emulator answers HTTP 501 NotImplemented; AWS answers 200 with an empty body.
4. transfer DescribeWebAppCustomization(WebAppId) -> on AWS this now answers HTTP 404 ResourceNotFoundException with message 'The WebApp <WebAppId> doesn''t have customization'; on the emulator the customization is still present because step 3 never took effect.
5. To see the raced form: run steps 2's update and step 3's delete concurrently from two threads released by a barrier — the outcome set must be a subset of {success, ConflictException}, but the emulator yields {update: success, delete: NotImplemented}.
