Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-product-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-product-lifecycle apigatewayv2-portal-multiple-isolation apigatewayv2-portal-product-multi-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ListPortalProducts|2xx->2xx|$.items[*]
Issue-Key: apigateway|ListPortalProducts|2xx->2xx|$.nextToken
Issue-Key: apigateway|GetPortalProduct|2xx->2xx|$.displayOrder
Issue-Related: 20261029T220031Z-apigateway-portal-product-and-vpc-link-timestamps-serialized.md
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway portal-product listings: ordering, nextToken and displayOrder diverge

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPortalProducts

## Description
The portal-product and portal collection endpoints diverge on pagination and ordering, and the portal-product body omits a defaulted member. Four causes, each with its own fixture step.

Cause 1 of 4: ListPortalProducts omits `nextToken`.
AWS: the response always carries a `nextToken` continuation token - the recording shows one present even when the page is the last one, so its presence (not its literal value) is the requirement; the value is an opaque server-generated token and differs per environment.
Emulator: `nextToken` absent.
Why: a paginating client that loops while nextToken is present terminates after one page on AWS-compatible code paths but cannot distinguish "no more pages" from "token not implemented", and SDK paginators that require the member fault.
Fix: the ListPortalProducts response projection / pagination state.
Traces: apigatewayv2-portal-product-lifecycle#task#3.

Cause 2 of 4: ListPortals returns its items in the wrong order.
AWS: portals are returned in a stable order (the recording lists the first-created portal at items[0]).
Emulator: items[0] and items[1] are transposed relative to the recording - the second-created portal is first.
Why: a client that reads items[0] as the oldest/first portal, or that diffs a listing between calls, gets the wrong resource. Only the ordering is the requirement here, not the generated portal ids in the payload.
Fix: the ListPortals collection ordering in the portal store.
Traces: apigatewayv2-portal-multiple-isolation#task#3.

Cause 3 of 4: GetPortalProduct omits `displayOrder`.
AWS: the portal-product body carries `displayOrder` with its empty default `{"contents": [], "productPageArns": []}` (recorded behavior only - the model declares the member, the default is from the recording).
Emulator: `displayOrder` absent.
Why: a client that reads product.displayOrder.productPageArns to render a product's page order dereferences a null.
Fix: the GetPortalProduct response projection - populate the empty displayOrder structure.
Traces: apigatewayv2-portal-product-lifecycle#task#2.

Cause 4 of 4: ListPortalProducts ordering plus the missing `nextToken`.
AWS: products are returned in creation order (the recording lists the first-created product at items[0]) and the response carries `nextToken`.
Emulator: items[0] and items[1] transposed, `nextToken` absent.
Why/Fix: as causes 1 and 2, in the ListPortalProducts collection ordering and pagination.
Traces: apigatewayv2-portal-product-multi-isolation#task#3.

## Evidence
Replay verdict `mismatch`. Details: `$.nextToken: missing (expected 'bnVsbCxTdGFydFBhZ2luYXRpbmdTaGFyZWRSZXNvdXJjZXM=', got None)`; `$.items[0]`/`$.items[1]` mismatch where the emulator's items[0] carries the SECOND created resource's displayName/description and items[1] the first; `$.displayOrder: missing (expected {'contents': [], 'productPageArns': []}, got None)`. Statuses matched (200) in all four clusters.

## Steps to Reproduce
Replay the corpus. Cause 1: create one portal product, then GET /v2/portalproducts (fixture apigatewayv2-portal-product-lifecycle step task#3) - no nextToken in the body. Cause 4: create two products A then B and list them (apigatewayv2-portal-product-multi-isolation#task#3) - B is returned at items[0].
