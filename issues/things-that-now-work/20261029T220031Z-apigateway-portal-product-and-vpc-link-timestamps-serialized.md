Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-multiple-product-pages-isolation
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-multiple-product-pages-isolation apigatewayv2-portal-product-error-paths apigatewayv2-portal-product-lifecycle apigatewayv2-portal-product-multi-isolation apigatewayv2-portal-product-page-lifecycle apigatewayv2-portal-product-sharing-policy apigatewayv2-portal-product-sharing-policy-roundtrip apigatewayv2-portal-with-included-portal-product apigatewayv2-portal-with-included-product apigatewayv2-product-page-lifecycle apigatewayv2-product-rest-endpoint-page-errors apigatewayv2-product-page-error-paths apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation apigatewayv2-vpc-link-tags-unicode-round-trip
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreatePortalProduct|2xx->2xx|$.lastModified
Issue-Key: apigateway|CreateProductPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|CreatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|CreatePortal|2xx->2xx|$.lastModified
Issue-Key: apigateway|ListPortals|2xx->2xx|$.items[*]
Issue-Key: apigateway|UpdatePortalProduct|2xx->2xx|$.lastModified
Issue-Key: apigateway|GetPortalProduct|2xx->2xx|$.lastModified
Issue-Key: apigateway|GetProductPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|UpdateProductPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|CreateVpcLink|2xx->2xx|$.createdDate
Issue-Key: apigateway|GetVpcLink|2xx->2xx|$.createdDate
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway portal, product and VPC-link timestamps serialized as epoch integers

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortalProduct

## Description
The same timestamp-serializer fault reaches the developer-portal resources (`lastModified`) and the VPC-link resource (`createdDate`): the emulator emits a Unix epoch integer where AWS emits an RFC3339 date-time string. On the portal resources the recorded strings carry sub-second precision (e.g. "2026-09-05T04:29:36.684875953Z"), so fractional seconds are part of the required form. Ten clusters, one per operation, so each can be re-replayed.

Cause 1 of 10: CreatePortalProduct.lastModified.
AWS: an ISO-8601 date-time string with fractional seconds. Smithy: the portal-product timestamp members target __timestampIso8601 (timestampFormat date-time).
Emulator: an epoch-seconds integer.
Why: SDK date-time deserialization fails or yields a wrong value, and the sub-second ordering a client uses to detect the newest revision is lost.
Fix: the REST-JSON timestamp writer used by the portal/product response encoders.
Traces: apigatewayv2-multiple-product-pages-isolation#setup#1.

Cause 2 of 10: CreateProductPage.lastModified.
AWS: ISO-8601 date-time string with fractional seconds.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-multiple-product-pages-isolation#task#2.

Cause 3 of 10: CreatePortal.lastModified.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#1.

Cause 4 of 10: ListPortals items[].lastModified.
AWS: ISO-8601 date-time string per listed portal.
Emulator: epoch integer, so the whole list element compares unequal.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#3.

Cause 5 of 10: UpdatePortalProduct.lastModified.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-product-lifecycle#task#4.

Cause 6 of 10: GetPortalProduct.lastModified.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-product-lifecycle#task#5.

Cause 7 of 10: GetProductPage.lastModified.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-product-page-lifecycle#task#3.

Cause 8 of 10: UpdateProductPage.lastModified.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-product-page-lifecycle#task#5.

Cause 9 of 10: CreateVpcLink.createdDate.
AWS: ISO-8601 date-time string. Smithy: the VpcLink CreatedDate member targets __timestampIso8601 with timestampFormat date-time.
Emulator: epoch integer.
Why/Fix: as cause 1, in the vpc-link response encoder.
Traces: apigatewayv2-vpc-link-tags-unicode-round-trip#task#1.

Cause 10 of 10: GetVpcLink.createdDate.
AWS: ISO-8601 date-time string.
Emulator: epoch integer.
Why/Fix: as cause 9.
Traces: apigatewayv2-vpc-link-tags-unicode-round-trip#task#2.

## Evidence
Replay verdict `mismatch`, detail `$.lastModified: value_mismatch (expected '<ISO-8601 with fractional seconds>', got <epoch integer>)` / `$.createdDate: value_mismatch (expected '<ISO-8601>', got <epoch integer>)` on all ten clusters; statuses matched. 30 recorded steps are affected.

## Steps to Reproduce
Replay the corpus and POST a portal product (fixture apigatewayv2-multiple-product-pages-isolation step setup#1): the 201 body's `lastModified` is a JSON number rather than a quoted RFC3339 string with fractional seconds. Same for the other nine fixture#step pairs.
