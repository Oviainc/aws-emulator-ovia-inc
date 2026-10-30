Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-multiple-product-pages-isolation
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-multiple-product-pages-isolation apigatewayv2-portal-product-page-errors apigatewayv2-portal-product-page-lifecycle apigatewayv2-product-page-error-paths apigatewayv2-product-page-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeleteProductPage|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteProductPage|2xx->4xx|$
Issue-Key: apigateway|DeleteProductPage|2xx->4xx|status
Issue-Commit: 05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d

# apigateway DeleteProductPage 204 contract diverges: no Content-Length and repeat delete answers 404

> _Note: this issue was identified against the code base at commit `05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteProductPage

## Description
Two net-new root causes in the DeleteProductPage empty-response path (DELETE /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}). Both concern the 204 No Content answer the operation is modeled to give: one never frames it with Content-Length, the other does not give it at all when the page is already gone. There are 2 causes and both must be closed before submitting.

Cause 1 of 2: the 204 answer omits the Content-Length: 0 header.
AWS: Smithy: DeleteProductPage @http {method: DELETE, uri: /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}, code: 204}, output Unit (no modeled headers), so for the header itself this is recorded behavior only - the recorded AWS responses answer 204 and carry an explicit `content-length` response header whose value is `0` for this empty body.
Emulator: 204 with no `content-length` header at all ($headers.content-length: missing, got None), status matching at 204.
Why: an HTTP client that closes out the message on an explicit zero length, rather than on 204's implicit no-body rule, can block waiting for a body or treat the response as malformed; a proxy that re-frames the response may also disagree with the origin about message end.
Fix: the REST-JSON response serializer branch for empty-output (Unit) operations - it must emit `Content-Length: 0` on a 204. This is the same serializer fault already filed for ten other ApiGatewayV2 delete operations in open issue `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md`; that issue's bundle is full at ten causes and does NOT carry a replay key for DeleteProductPage, so this cause is filed here with its own key.
Traces: apigatewayv2-multiple-product-pages-isolation#task#5 (cluster also covers apigatewayv2-multiple-product-pages-isolation#task#6, apigatewayv2-portal-product-page-errors#task#3, apigatewayv2-portal-product-page-lifecycle#task#7, apigatewayv2-product-page-error-paths#task#5, apigatewayv2-product-page-lifecycle#task#7).

Cause 2 of 2: DeleteProductPage is not idempotent - deleting an already-deleted page is refused 404 instead of answering 204.
AWS: Smithy: DeleteProductPage @http code 204, output Unit, modeled errors AccessDeniedException(403), BadRequestException(400), NotFoundException(404), TooManyRequestsException(429). The model permits NotFoundException but the recording shows AWS does not use it for an absent product page: a second DELETE of the same ProductPageId answers 204 with an empty JSON body ({}). Deletion of a product page is idempotent on AWS.
Emulator: 404 with body {"__type":"NotFoundException","message":"Invalid ProductPage identifier specified"} - the status, the error code and the message are all emitted where AWS emits an empty 204. (Even where a NotFoundException is correct for this surface, `Invalid ProductPage identifier specified` is not AWS's wording; AWS words the absent-page refusal for the sibling read as the fixed text `ProductPage not found`, with no variable part - see open issue `20261029T220031Z-apigateway-notfoundexception-messages-do-not-match-aws-wording.md`, which owns the GetProductPage wording but not this delete-path status divergence.)
Why: teardown, retry and reconciliation paths that delete a product page twice - or that race a concurrent delete - succeed against AWS and fail against the emulator, so a client's cleanup aborts on a resource that is already in the desired state.
Fix: the DeleteProductPage handler's absent-resource branch - a missing ProductPageId under an existing PortalProduct must be answered 204 with an empty body, not raised as NotFoundException. This is the same idempotency fault already filed for the sibling resource (DeleteProductRestEndpointPage, cause 4) in open issue `20261029T220031Z-apigateway-managed-routes-route-settings-deletion-and-repeat.md`; that issue carries no replay key for DeleteProductPage, so this cause is filed here.
Traces: apigatewayv2-product-page-error-paths#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). Both clusters have verdict `mismatch`, empty `stub_path` and empty `owned_by` (i.e. no open issue carries their replay key).

Cause 1 - divergence_key `service=apigateway|DeleteProductPage|status=204->204|keys=$headers.content-length`, expected_status 204, actual_status 204, detail: `$headers.content-length: missing (expected '0', got None)`. 6 recorded steps affected.

Cause 2 - divergence_key `service=apigateway|DeleteProductPage|status=204->404|keys=$,status`, expected_status 204, actual_status 404, detail: `status: expected 204, got 404` and `$: value_mismatch (expected {}, got {'__type': 'NotFoundException', 'message': 'Invalid ProductPage identifier specified'})`. 1 recorded step affected.

In cause 2 the diverging requirement is the whole response: status 204, no error code, empty body `{}`. The emulator's `404` / `NotFoundException` / `Invalid ProductPage identifier specified` are all divergent; of that message, every word is fixed emulator-authored text (it interpolates nothing from the request), which is why no recorded id or name appears in it.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Cause 1 - fixture apigatewayv2-multiple-product-pages-isolation, step task#5: create a portal product, create a product page under it, then DELETE /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId} and dump the raw response headers. The emulator answers 204 with no Content-Length header; the recording has `content-length: 0`.
3. Cause 2 - fixture apigatewayv2-product-page-error-paths, step task#6: create a product page (task#4), DELETE it once (task#5, 204), then issue the identical DELETE a second time. The emulator answers 404 with `{"__type":"NotFoundException","message":"Invalid ProductPage identifier specified"}`; the recording answers 204 with an empty body.

## Resolution

Cause 1 (`content-length: 0` on the empty 204): the service now states that framing on every
answer it serializes with no content, at the one place every modelled response passes through —
`Service::override_protocol` in `src/service.rs` serves the model's own protocol with a `204`
emptied of its body and framed `content-length: 0`. It therefore holds for every operation of
this model whose success is 204 over `Unit`, not only this delete. CAVEAT: a client reached over
HTTP/1.1 still does not see the header, because the HTTP writer the vendored transport serves
with removes `content-length` from a `204` — that writer is under the read-only vendored tree, so
the wire-level half of this cause is a platform limitation rather than something this service can
state its way out of. The serializer seam is pinned by `tests/all/empty_answer_framing.rs`.

Cause 2 (the repeat delete): deleting a page of a portal product the account holds is now
idempotent — a product that holds no page under the addressed id is already in the state the call
asks for, so it is answered with the same empty 204 as the delete that removed one. Both page
kinds bound under a portal product follow the rule (`DeleteProductPage` and
`DeleteProductRestEndpointPage`), as `DeletePortalProductSharingPolicy` already did for the policy
it clears. A `PortalProductId` the account holds no product under is still refused 404
`NotFoundException`, now naming the identifier that resolved to nothing. Pinned by
`tests/all/absent_page_delete_idempotence.rs`.
