Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-iam-auth-agreement-endpoint-method-discovery
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-iam-auth-agreement-endpoint-method-discovery apigateway-iam-auth-resource-tree-paginated-walk
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetResources|2xx->2xx|$.item[id="5mw7f4baaa"].resourceMethods.GET.methodIntegration.responseTransferMode
Issue-Key: apigateway|GetMethod|2xx->2xx|$.methodIntegration.responseTransferMode
Issue-Key: apigateway|GetResources|2xx->2xx|$.limit
Issue-Key: apigateway|GetResources|2xx->2xx|$.position
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# API Gateway method representations omit responseTransferMode and GetResources mis-echoes its page envelope

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Four net-new root causes in how the API Gateway REST control plane serializes method/integration representations and the GetResources page envelope. All four are apigateway and are filed as one bundle; a fixer must close ALL FOUR before submitting.

Cause 1 of 4: GetResources' embedded methodIntegration omits responseTransferMode.
AWS: Integration carries a modeled `responseTransferMode` member (Smithy: Integration.responseTransferMode -> ResponseTransferMode, values BUFFERED | STREAM; GetResources output shape Resources, items jsonName `item`, each Resource carrying resourceMethods -> Method -> methodIntegration -> Integration). The recording shows AWS reporting `BUFFERED` on an integration created without specifying the member, i.e. BUFFERED is the server-side default and is always present on the representation; STREAM appears only when configured.
Emulator: the member is absent from every embedded methodIntegration returned by GetResources (status 200 is correct).
Why: a client reading the resource tree to discover how a method streams its response sees no transfer mode at all and cannot distinguish BUFFERED from unset; round-tripping the representation back through PutIntegration loses the mode.
Fix: the Integration serializer in the apigateway service crate (shared by the resource-tree embedding) must default and emit responseTransferMode.
Traces: apigateway-iam-auth-agreement-endpoint-method-discovery#task#8.

Cause 2 of 4: GetMethod's methodIntegration omits responseTransferMode.
AWS: same modeled member, reached through GetMethod (Smithy: GetMethod output shape Method, member methodIntegration -> Integration; Integration.responseTransferMode -> ResponseTransferMode). The recording shows `BUFFERED` present on the integration of a method created without specifying it.
Emulator: $.methodIntegration.responseTransferMode is absent on a 200 GetMethod response.
Why: the same loss on the single-method read path, which is what SDK helpers and infrastructure tools use to reconcile desired vs actual integration state - the member reads as unmanaged and drifts silently.
Fix: same Integration serializer / Method assembly in the apigateway service crate; it must be fixed on the GetMethod path as well as the GetResources path.
Traces: apigateway-iam-auth-agreement-endpoint-method-discovery#task#10.

Cause 3 of 4: GetResources reports the page size as 0 instead of the requested limit.
AWS: GetResources is paginated with pageSize `limit` and inputToken/outputToken `position` (Smithy: GetResources paginated {inputToken: position, outputToken: position, items: items, pageSize: limit}; `limit` is an httpQuery NullableInteger). The Resources output shape models only items and position, so the echoed `limit` in the response body is recorded behavior only: the recording shows the body echoing the limit the caller sent when a limit was supplied.
Emulator: emits `limit: 0` on that same first page - a value the caller never sent and which, read as a page size, means no page size at all.
Why: a paginating client that trusts the echoed page size computes a zero-sized window, so a hand-rolled pager either stalls or mis-slices the resource tree.
Fix: the GetResources response assembly / page envelope in the apigateway service crate - echo the request's limit rather than a zero default.
Traces: apigateway-iam-auth-resource-tree-paginated-walk#task#5.

Cause 4 of 4: GetResources emits position and limit on the final page, where AWS omits both.
AWS: `position` is the pagination token and the recording shows it ABSENT from the last page of the walk - that absence is how a client knows the walk is complete - and no `limit` echoed there either (recorded behavior only; Resources models no limit member at all).
Emulator: the final page carries both a `position` token (a resource id) and `limit`, so there is no terminal page.
Why: a client that loops while position is present never terminates - it re-requests with the returned token forever, or re-reads the final page - which is a non-terminating pagination loop rather than a cosmetic difference. The returned token's concrete value is environment-specific; the requirement is that the member is absent once the collection is exhausted.
Fix: the GetResources pagination/terminal-page logic in the apigateway service crate - stop emitting position (and limit) once the last page is served.
Traces: apigateway-iam-auth-resource-tree-paginated-walk#task#9.

Related open issues that do NOT cover these steps (the replay report attributes no owner to any of the four): 20261029T200808Z-apigateway-api-gateway-integration-resource-representation-drops-responsetransfermode-and.md (same member, different operations/steps) and 20261101T082850Z-apigateway-api-gateway-rest-list-envelopes-empty-collections-position.md (list envelopes, different steps).

## Evidence
Cause 1 (GetResources, apigateway-iam-auth-agreement-endpoint-method-discovery#task#8): status 200 both sides; $.item[...].resourceMethods.GET.methodIntegration.responseTransferMode expected 'BUFFERED', actual missing.
Cause 2 (GetMethod, apigateway-iam-auth-agreement-endpoint-method-discovery#task#10): status 200 both sides; $.methodIntegration.responseTransferMode expected 'BUFFERED', actual missing.
Cause 3 (GetResources, apigateway-iam-auth-resource-tree-paginated-walk#task#5): status 200 both sides; $.limit expected 1 (the requested page size), actual 0.
Cause 4 (GetResources, apigateway-iam-auth-resource-tree-paginated-walk#task#9): status 200 both sides; $.position expected absent, actual present (a resource id); $.limit expected absent, actual 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Causes 1 and 2: replay fixture apigateway-iam-auth-agreement-endpoint-method-discovery, steps task#8 (GetResources) and task#10 (GetMethod) - create a resource + method + integration without setting responseTransferMode, then read it back and inspect methodIntegration.responseTransferMode.
3. Cause 3: replay fixture apigateway-iam-auth-resource-tree-paginated-walk, step task#5 - GetResources with limit=1; inspect the echoed $.limit.
4. Cause 4: same fixture, step task#9 - walk GetResources to the end of the resource tree; inspect $.position and $.limit on the final page.
