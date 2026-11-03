Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-create-update-delete-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation apigatewayv2-portal-with-included-portal-product apigatewayv2-portal-with-included-product apigatewayv2-portal-disable-before-publish-conflict apigatewayv2-portal-preview-publish-disable
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|GetPortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# apigateway portal responses return a host:port URL in endpointConfiguration.portalDefaultDomainName

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortal

## Description
Every portal-returning operation serializes endpointConfiguration.portalDefaultDomainName as an emulator listener address with a port and a URL path instead of a bare DNS domain name. One root cause in the portal representation builder, surfacing on three operations.

Cause 1 of 3: CreatePortal (fixture apigatewayv2-portal-create-update-delete-lifecycle, step task#1).
AWS: Smithy (apigatewayv2 model) CreatePortalResponse.endpointConfiguration -> EndpointConfigurationResponse.PortalDefaultDomainName, target __stringMin3Max256, @required, doc 'The portal default domain name. This domain name is generated and managed by API Gateway.' It is a DNS host name only: label.label...tld, with no scheme, no ':port' and no '/path' segment (the recorded AWS responses carry a hostname of that form, which is why the fixture asserts the member's shape rather than a literal value).
Emulator: returns a value of the form '<host>:<port>/_aws/portal/<portalId>' — i.e. an authority plus a path — which fails the domain-name shape assertion.
Why: a client builds the portal URL by prefixing 'https://' to this member and/or uses it as a CNAME/Route 53 alias target. A value carrying a port and a path yields an unusable URL and cannot be used as a DNS name at all, and any client-side hostname validation rejects it.
Fix: the portal representation serializer/state that mints the default domain name for a portal — mint a hostname-only value (host labels only) and keep the port/path, if the emulator needs them to serve the portal, out of this member.

Cause 2 of 3: GetPortal (fixture apigatewayv2-portal-create-update-delete-lifecycle, step task#2) — same member, same malformed value read back for an existing portal. Smithy GetPortalResponse.endpointConfiguration uses the same EndpointConfigurationResponse shape. 10 steps across 8 portal fixtures are affected.

Cause 3 of 3: UpdatePortal (fixture apigatewayv2-portal-create-update-delete-lifecycle, step task#4) — same member, same malformed value after a PATCH. Smithy UpdatePortalResponse.endpointConfiguration uses the same shape.

No open issue carries these replay keys; the nearest open portal issues (20261030T153728Z-apigateway-portal-product-page-delete-and-list-responses-diverge.md, 20261031T154120Z-apigateway-listportals-portal-summaries-omit-the-preview-member.md) are about portal PRODUCT pages and ListPortals summaries, not portalDefaultDomainName.

## Evidence
Replay divergence clusters (service=apigateway):
- CreatePortal, status 201 expected/201 actual, '$.endpointConfiguration.portalDefaultDomainName: value_mismatch (expected <shaped-not-asserted>, got "127.0.0.1:44539/_aws/portal/z1hvlts8lq")' — 5 affected steps.
- GetPortal, 200/200, same member, same malformed form — 10 affected steps.
- UpdatePortal, 200/200, same member, same malformed form — 2 affected steps.
The host and port in the recorded actual value are per-run and are NOT the requirement; the requirement is that the member be a bare DNS host name with no port and no path.

## Steps to Reproduce
1. Boot the emulator and replay fixture apigatewayv2-portal-create-update-delete-lifecycle.
2. Step task#1 (CreatePortal POST /v2/portals) — inspect endpointConfiguration.portalDefaultDomainName in the 201 body: it contains '<host>:<port>/_aws/portal/<portalId>'.
3. Step task#2 (GetPortal GET /v2/portals/{portalId}) and step task#4 (UpdatePortal PATCH /v2/portals/{portalId}) return the same malformed value.
4. Expected: a bare domain name (host labels only, length 3..256), no port, no path.
