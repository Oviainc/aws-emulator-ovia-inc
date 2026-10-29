Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered apigatewayv2.e2e-custom-domain-mapping-invoke-flow apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.e2e-custom-domain-mapping-invoke-flow|run#10|KeyError|gw_zone = cfg["HostedZoneId"]
Issue-Key: apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value|_poll_http#21|TimeoutError|raise TimeoutError(f"{description} ({url}): the run budget ended first; not graded")
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# apigatewayv2: 3 net-new root causes — the 10 MiB HTTP-API payload gate never answers, DomainNameConfiguration omits HostedZoneId, and a REQUEST-authorizer-gated route never serves

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainName

## Description
Three distinct net-new root causes in the apigatewayv2 emulator, found by replaying the behavioral suite. Each is numbered below and carries its own failing check ids; all three must be closed before this issue is resubmitted.

--------------------------------------------------------------------
Cause 1 of 3: the HTTP-API request-payload limit is never enforced, and the invoke never answers at all

AWS:      An HTTP API caps the request payload at 10 MiB (10 * 1024 * 1024 = 10485760 bytes) and the limit is not adjustable. A body of exactly the limit is delivered to the integration, which answers it normally (2xx from the backend). A body of one byte more is refused by the gateway itself, before the integration is invoked, with HTTP status 413 — the integration must observe no invocation for the refused request. recorded behavior only: the apigatewayv2 Smithy model declares no payload-size member or modelled error for this — it is a data-plane service quota, so the requirement is the status (413 for limit+1, backend 2xx for exactly limit) and the non-delivery, taken from the recorded data-plane responses. The byte count is the fixed part of the rule; the API id, stage name and route path in the probe URL are values from the request.
Emulator: the invoke of the deployed stage produces no response whatsoever for either leg. The check reaches the harness cap with no status at all — "Check timed out after 60.0s" — so neither the 413 nor the at-limit 2xx is ever produced, and the boundary is unverifiable.
Why:      A client uploading a body near the limit gets a hang instead of a verdict. It cannot distinguish "payload too large" (413, retry with a smaller body, a client bug) from "gateway unreachable" (retry the same body), so upload clients and SDK retry policies behave wrongly, and oversized bodies are never rejected at the edge.
Fix:      The execute-api HTTP data-plane request path: a body-size gate must run ahead of integration dispatch, emitting 413 for over-limit bodies and forwarding bodies at exactly the limit.
Traces:   apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered. This check shares the non-serving execute-api data plane with the open issue 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md, which does not carry this check's replay key and does not state the 10 MiB / 413 requirement; that issue does not cover this cause.

--------------------------------------------------------------------
Cause 2 of 3: DomainNameConfiguration is returned without HostedZoneId

AWS:      Smithy: DomainNameConfiguration.HostedZoneId (target __string, jsonName hostedZoneId) — documented as "The Amazon Route 53 Hosted Zone ID of the endpoint." For a REGIONAL custom domain, once DomainNameStatus reaches AVAILABLE, CreateDomainName / GetDomainName / GetDomainNames return the DomainNameConfigurations entry with BOTH ApiGatewayDomainName and HostedZoneId populated. HostedZoneId is the fixed per-region, per-endpoint-type API Gateway alias-target zone id; it is what a caller needs to create the Route 53 ALIAS record that makes the custom domain resolvable. The member name and its presence are the requirement; the particular zone id string and the domain name are values from the region and the request.
Emulator: GetDomainName returns a DomainNameConfigurations[0] that reaches DomainNameStatus=AVAILABLE and carries ApiGatewayDomainName, but has no hostedZoneId key at all. Reading it raises KeyError: 'HostedZoneId' (check_apigatewayv2_e2e_custom_domain_mapping_invoke_flow.py line 50, gw_zone = cfg["HostedZoneId"]).
Why:      A client cannot build the Route 53 ALIAS record for the custom domain, because the alias target's hosted-zone id is simply absent from the response. Every CloudFormation / CDK / Terraform pattern that wires an apigatewayv2 custom domain to Route 53 reads this member, so each one fails on a response that otherwise looks complete.
Fix:      The CreateDomainName / UpdateDomainName state that materialises DomainNameConfiguration must set a hosted-zone id per endpoint type, and the GetDomainName / GetDomainNames serializers must emit it.
Traces:   apigatewayv2.e2e-custom-domain-mapping-invoke-flow. The open issue 20261026T023803Z-apigatewayv2-custom-domain-and-api-mapping-control-plane.md covers other custom-domain defects and never mentions HostedZoneId; it does not cover this cause.

--------------------------------------------------------------------
Cause 3 of 3: an HTTP-API route gated by a REQUEST authorizer never serves, so the authorizer result cache cannot be keyed at all

AWS:      For an HTTP API route protected by a REQUEST (Lambda) authorizer with an IdentitySource and AuthorizerResultTtlInSeconds greater than zero, API Gateway invokes the authorizer, then caches the decision keyed on the concatenated identity-source VALUES. Two requests inside the TTL carrying the same identity-source value invoke the authorizer exactly once; a request carrying a different identity-source value is a cache miss and invokes it again. Every allowed request is answered by the backend integration with its own status (200 here). recorded behavior only for the cache-keying rule — Smithy models Authorizer.IdentitySource and Authorizer.AuthorizerResultTtlInSeconds but not the keying semantics; the requirement is the recorded one-invocation-per-distinct-identity-source-value count and the 200 the backend returns.
Emulator: the authorizer-protected route never answers. Polling it exhausts the run budget with no response: TimeoutError: "the authorizer-protected route serving its backend (https://<host>/_aws/execute-api/<apiId>/$default/guarded): the run budget ended first; not graded". The host, port and api id there are per-run values; the invariant is that no status is ever returned.
Why:      A client behind a REQUEST authorizer gets a hang rather than 200 or 401/403, so authorization cannot be observed at all, and nothing that depends on the authorizer cache (invocation counts, per-identity isolation, TTL behaviour) can be relied on.
Fix:      The execute-api data-plane authorizer pipeline: invoke the REQUEST authorizer, cache its decision keyed on the identity-source values, then dispatch to the route's integration.
Traces:   apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value. The open issue 20261025T174820Z-apigatewayv2-an-http-api-route-gated-by-a.md concerns authorizer-cache TTL expiry and does not carry this check's replay key; it does not cover this cause.

## Evidence
Cause 1 — expected: POST of a 10485760-byte body answered by the integration (backend 2xx); POST of 10485761 bytes refused by the gateway with HTTP 413 and no integration invocation. Actual: no response at all on either leg; "[mismatch] apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered — Error: Check timed out after 60.0s".

Cause 2 — expected: GetDomainName -> DomainNameConfigurations[0] contains hostedZoneId (the Route 53 alias-target zone id of the endpoint) alongside apiGatewayDomainName, per Smithy DomainNameConfiguration.HostedZoneId. Actual: the member is absent from the response; the check raised `KeyError: 'HostedZoneId'` at check_apigatewayv2_e2e_custom_domain_mapping_invoke_flow.py line 50 after DomainNameStatus had already become AVAILABLE and ApiGatewayDomainName had been read successfully.

Cause 3 — expected: GET of the authorizer-protected route returns 200 from the backend, with the authorizer invoked once per distinct identity-source value within the TTL. Actual: no status ever returned — `TimeoutError: the authorizer-protected route serving its backend (https://127.0.0.1:<port>/_aws/execute-api/<apiId>/$default/guarded): the run budget ended first; not graded` (check_apigatewayv2_request_authorizer_cache_is_keyed_per_identity_source_value.py line 139).

## Steps to Reproduce
Cause 1: create an HTTP API with a proxy route and a deployed stage; POST a body of exactly 10*1024*1024 bytes (expect the backend's 2xx) and then a body of 10*1024*1024+1 bytes (expect gateway 413, integration not invoked). Replay check apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered.

Cause 2: ImportCertificate a certificate for a domain, CreateDomainName with DomainNameConfigurations=[{CertificateArn, EndpointType: REGIONAL}], poll GetDomainName until DomainNameConfigurations[0].DomainNameStatus == AVAILABLE, then read DomainNameConfigurations[0]["HostedZoneId"]. Replay check apigatewayv2.e2e-custom-domain-mapping-invoke-flow.

Cause 3: create an HTTP API with a REQUEST Lambda authorizer (IdentitySource set, AuthorizerResultTtlInSeconds > 0) on a route, deploy a stage, then GET the guarded route twice with the same identity-source value and once with a different value. Replay check apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value.
