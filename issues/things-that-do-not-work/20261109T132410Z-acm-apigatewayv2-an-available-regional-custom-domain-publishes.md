Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it
Issue-Key: acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it|_request#23|PublishedUrlRejected|with socket.create_connection(ctx.endpoint_host(gateway_host, 443), timeout=15) as raw:
Issue-Commit: cda81fc610349487f102bcc6f8ad570fe265d90b

# acm__apigatewayv2: an AVAILABLE REGIONAL custom domain publishes ApiGatewayDomainName as a real-AWS d-*.execute-api.<region>.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `cda81fc610349487f102bcc6f8ad570fe265d90b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
One net-new root cause (Cause 1 of 1).

Related open filings that do NOT cover this replay: 20261025T210904Z-acm-apigatewayv2-a-regional-custom-domain-never-presents.md (the certificate was never presented on the wire) — this replay no longer gets as far as a TLS attempt, it is refused in the harness's published-address gate, so the suite does not mark the check OWNED. 20261109T124858Z-apigatewayv2-cognito-idp-lambda-an-http-api-s.md and 20261109T120740Z-iot-the-published-data-plane-address-is-a.md describe the same class of defect but are filed under other service headings and do not carry this check id.

Cause 1 of 1: apigatewayv2 publishes a REGIONAL custom domain's ApiGatewayDomainName as an AWS-owned execute-api hostname instead of an address the emulator answers on.
AWS: Smithy (apigatewayv2) DomainNameConfiguration.ApiGatewayDomainName (jsonName apiGatewayDomainName) — "A domain name for the API", published together with HostedZoneId and DomainNameStatus; CreateDomainName/GetDomainName return it so a customer can point DNS (an ALIAS/CNAME) for their custom domain at it. Recorded AWS behavior: once DomainNameConfigurations[0].DomainNameStatus is AVAILABLE, a TLS connection to ApiGatewayDomainName:443 with SNI = the custom domain completes under the configured SecurityPolicy and presents the ACM certificate named by CertificateArn; after UpdateDomainName swaps CertificateArn, the same address presents the new leaf. The address is therefore required to be reachable.
Emulator: GetDomainName reports DomainNameStatus AVAILABLE and ApiGatewayDomainName of the form d-<id>.execute-api.<region>.amazonaws.com (replay: 'd-22ftwv00cl.execute-api.us-west-2.amazonaws.com' — the d-id is a per-run value; the defect is the domain). That host is not one of the emulator's own hosts, so the harness refuses it: PublishedUrlRejected "endpoint_host was given d-<id>.execute-api.<region>.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address".
Why: the only address API Gateway gives a customer for a REGIONAL custom domain is unusable — it either escapes to real AWS or does not resolve. Certificate presentation under the custom domain, the SecurityPolicy that governs the handshake, and certificate rotation via UpdateDomainName are all unobservable, while the control plane reports a fully provisioned AVAILABLE domain.
Fix: the apigatewayv2 custom-domain handler/state that mints and serializes DomainNameConfiguration.ApiGatewayDomainName (CreateDomainName, GetDomainName, UpdateDomainName) must publish the emulator's own reachable host[:port]; the custom-domain TLS front door must then terminate on it with the ACM certificate named by CertificateArn.
Traces: acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm__apigatewayv2 0/1 (0%), 1 failed.

[mismatch] acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it:
  File "context.py", line 619, in endpoint_host -> self._published_target(f"{published_host}:{port}", "endpoint_host")
  simparity.published.PublishedUrlRejected: endpoint_host was given d-22ftwv00cl.execute-api.us-west-2.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

The check imports two ACM leaves, calls apigw.create_domain_name(DomainName=<domain>, DomainNameConfigurations=[{CertificateArn: old_arn, EndpointType: 'REGIONAL', SecurityPolicy: 'TLS_1_2'}]), polls get_domain_name until DomainNameStatus == 'AVAILABLE', takes gateway_host = config['ApiGatewayDomainName'] and then does socket.create_connection(ctx.endpoint_host(gateway_host, 443)) with SNI/Host = the custom domain. Expected: the handshake completes, the presented leaf equals the imported certificate, and after UpdateDomainName the same host presents the rotated leaf. Actual: the published ApiGatewayDomainName is an AWS execute-api host the emulator does not serve, so no handshake is attempted.

Smithy consulted: apigatewayv2 DomainNameConfiguration (ApiGatewayDomainName, HostedZoneId, DomainNameStatus, SecurityPolicy, CertificateArn).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; acm__apigatewayv2.regional-custom-domain-presents-the-imported-certificate-and-rotation-swaps-it reproduces.
2. acm.import_certificate a leaf for the custom domain; poll until Status == ISSUED.
3. apigatewayv2.create_domain_name(DomainName=<domain>, DomainNameConfigurations=[{'CertificateArn': <arn>, 'EndpointType': 'REGIONAL', 'SecurityPolicy': 'TLS_1_2'}]).
4. apigatewayv2.get_domain_name(DomainName=<domain>) until DomainNameConfigurations[0].DomainNameStatus == 'AVAILABLE', then read ApiGatewayDomainName: the emulator returns d-<id>.execute-api.<region>.amazonaws.com.
5. Open TLS to that host on 443 with SNI = the custom domain. AWS completes the handshake and presents the imported certificate; the emulator published an address it does not serve.
6. Re-replay the check id in trace_ids to confirm the fix.
