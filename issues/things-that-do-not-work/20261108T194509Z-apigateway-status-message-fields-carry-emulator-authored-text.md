Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-product-rest-endpoint-page-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-product-rest-endpoint-page-lifecycle apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.statusException.message
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-4a241703"].vpcLinkStatusMessage
Issue-Commit: bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c

# apigateway status-message fields carry emulator-authored text instead of AWS wording

> _Note: this issue was identified against the code base at commit `bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProductRestEndpointPage

## Description
Two human-readable status/exception message members in API Gateway resource representations are filled with sentences the emulator authored itself rather than the text AWS returns for that state. In both cases the HTTP status and the surrounding structure are right; only the message member's text diverges, so a client that surfaces or matches on these strings sees something AWS never emits. 2 causes.

Cause 1 of 2: GetProductRestEndpointPage leaks a request-validation sentence into statusException.message. AWS: Smithy apigatewayv2 GetProductRestEndpointPage -> GetProductRestEndpointPageResponse.statusException (StatusException{exception: __stringMin1Max256, message: __stringMin1Max2048 "The error message"}) alongside status (Status enum AVAILABLE|IN_PROGRESS|FAILED). The recording asserts statusException.message by shape (the corpus records it as <shaped-not-asserted>), i.e. AWS returns a message describing why that endpoint page's status is not AVAILABLE, of a shape the emulator's value does not satisfy; recorded behavior only for its exact fixed text. Note that 'Invalid restEndpointIdentifier stage or restApiId' is the text AWS returns as a 400 BadRequestException from CreateProductRestEndpointPage when the restEndpointIdentifier names an unknown restApiId/stage (see the recorded CreateProductRestEndpointPage 400 covered by open issue 20261029T220031Z-apigateway-control-plane-accepts-invalid-input-and-rejects.md) - it is not a page-status message. Emulator: GET /v2/portalproducts/{portalProductId}/productrestendpointpages/{id} answers 200 with statusException.message = 'Invalid restEndpointIdentifier stage or restApiId', failing the recorded shape assertion on that member. Hypothesis: the handler defers the create-time validation of restEndpointIdentifier and then re-uses that same validation sentence as the page's status exception. Why: a client reading statusException to report why a product REST endpoint page is unavailable gets API Gateway's request-validation wording for a call that succeeded, and any code keyed on that message text (or on its shape/length) behaves differently against real AWS. Fix: GetProductRestEndpointPage's response projection / the stored page state that populates statusException - the status exception must be derived from the page's own status, not from the create-request validator's message. Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#7 and #task#9 (2 steps).

Cause 2 of 2: GetVpcLinks reports the wrong vpcLinkStatusMessage for an available VPC link. AWS: Smithy apigatewayv2 VpcLink.VpcLinkStatusMessage (jsonName vpcLinkStatusMessage, StringWithLengthBetween0And1024, 'A message summarizing the cause of the status of the VPC link'); recorded behavior for an AVAILABLE link is the fixed sentence 'VPC link is ready to route traffic' - entirely fixed text, with no value taken from the request or from state. Emulator: GET /v2/vpclinks returns items[].vpcLinkStatusMessage = 'The VPC link is available for use by an integration.' Why: a client (or a waiter/poller) that matches the status message to decide a VPC link is usable never sees AWS's sentence, and any UI echoing the field shows wording AWS does not produce. Fix: the VPC-link state/serializer that mints vpcLinkStatusMessage per VpcLinkStatus value. Traces: apigatewayv2-vpc-link-errors#task#4. Related but not covering: open issue 20261029T200808Z-apigateway-api-gateway-tag-vpc-link-stage-and-import.md covers CreateVpcLink/DeleteVpcLink refusal messages, not vpcLinkStatusMessage on a successful read.

## Evidence
Cause 1 - fixture apigatewayv2-product-rest-endpoint-page-lifecycle, step task#7 (also task#9), GetProductRestEndpointPage, status 200 expected and 200 returned: $.statusException.message value_mismatch (expected <shaped-not-asserted>, i.e. AWS's page-status message shape, got 'Invalid restEndpointIdentifier stage or restApiId'). Fixed text vs values: the emulator's string is wholly fixed emulator text and contains no request/state values; AWS's member is the page's own status message.
Cause 2 - fixture apigatewayv2-vpc-link-errors, step task#4, GetVpcLinks, status 200 expected and 200 returned: $.items[name="sim2real-wire-vpclink-..."].vpcLinkStatusMessage value_mismatch (expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.'). The expected message is 100% fixed text; the only request/state-derived part of the step is the link's name used to select the item, not the message.
Both clusters are reported by replay_wire_corpus with owned_by: [] and stub_path: "" (no open issue carries their replay key, no unimplemented operation blocks them).

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus) against the emulator built from this checkout.
2. Cause 1: create a portal product and a product REST endpoint page (fixture apigatewayv2-product-rest-endpoint-page-lifecycle setup), then GET /v2/portalproducts/{portalProductId}/productrestendpointpages/{productRestEndpointPageId} (steps task#7 and task#9) and inspect $.statusException.message - it must be the page's status message, not the create-time 'Invalid restEndpointIdentifier stage or restApiId' validation sentence.
3. Cause 2: create a VPC link (fixture apigatewayv2-vpc-link-errors), then GET /v2/vpclinks (step task#4) and compare items[].vpcLinkStatusMessage against 'VPC link is ready to route traffic'.
