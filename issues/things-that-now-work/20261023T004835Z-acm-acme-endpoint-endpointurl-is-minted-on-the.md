Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-endpoint-directory-url-immutable-server-generated acm.acme-endpoint-directory-url-managed-by-service acm.acme-endpoint-server-managed-directory-url acm.describe-acme-endpoint-returns-server-managed-urls acm.acme-directory-serves-the-rfc8555-resource-map-and-single-use-nonces
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.acme-endpoint-directory-url-immutable-server-generated|_assert_directory_url#7|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.acme-endpoint-directory-url-managed-by-service|_assert_directory_url#6|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.acme-endpoint-server-managed-directory-url|_assert_directory_url#12|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.describe-acme-endpoint-returns-server-managed-urls|_assert_directory_url#11|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.acme-directory-serves-the-rfc8555-resource-map-and-single-use-nonces|run#4|AssertionError|assert status == 200, (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# ACM ACME endpoint: EndpointUrl is minted on the wrong host, and the directory URL it advertises is not served at all (2 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAcmeEndpoint

## Description
Two distinct root causes in the ACME-endpoint surface of ACM. Both are about the server-managed URL CreateAcmeEndpoint/DescribeAcmeEndpoint hand back for an ACME endpoint. Neither is covered by an open issue (the filed ACME stub issues name CreateAcmeDomainValidation, UpdateAcmeEndpoint, ListAcmeEndpoints, ListAcmeAccounts, DeleteAcmeEndpoint, TagResource and CreateAcmeExternalAccountBinding; the 'seven modeled operations answer HTTP 501' issue names DescribeAcmeAccount, ListAcmeEndpoints and GetAcmeExternalAccountBindingCredentials). DescribeAcmeEndpoint itself DOES answer here, so this is a fidelity divergence, not a stub.

Cause 1 of 2: the server-generated EndpointUrl is not on ACM's ACME enrollment host.
AWS: Smithy com.amazonaws.acm#AcmeEndpoint declares EndpointUrl (String, 'The URL of the ACME endpoint') and AcmeEndpointArn (pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$); EndpointUrl is server-managed and is NOT a member of CreateAcmeEndpointRequest or UpdateAcmeEndpointRequest, so a client can never set it. Recorded behavior (four independent blessed checks): the value ACM returns is hosted on the ACM ACME enrollment host for the region — the checks grade it as 'must be on acm-acme-enroll.<region>.<domain>' — and the same value is returned, unchanged, by every subsequent DescribeAcmeEndpoint.
Emulator: the URL is minted as 'https://<endpoint-uuid>.acme.<region>.amazonaws.com/directory' — the endpoint id is used as a host label and the host is not the ACME enrollment host at all. Status and shape are otherwise fine (the member is present and stable).
Why: an ACME client (certbot, lego, acme.sh) is configured with exactly this URL as its ACME server. Pointing it at a host of a different form means TLS/SNI and any host-based routing or allow-list a customer built around acm-acme-enroll.<region> does not match, so the client cannot enrol; a test that asserts the enrollment host passes against AWS and fails here.
Fix: the EndpointUrl generator in the ACME-endpoint state/handlers of the ACM service crate (services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_endpoint.rs and the AcmeEndpoint projection used by describe_acme_endpoint.rs) — mint the URL on the region's ACME enrollment host instead of embedding the endpoint uuid as a sub-domain label.
Traces: acm.acme-endpoint-directory-url-immutable-server-generated, acm.acme-endpoint-directory-url-managed-by-service, acm.acme-endpoint-server-managed-directory-url, acm.describe-acme-endpoint-returns-server-managed-urls.

Cause 2 of 2: nothing is served at the URL the emulator advertises — the ACME directory resource does not exist.
AWS: recorded behavior only for the exact body (the grading check for this leg is a DRAFT/unblessed check, so its RFC 8555 member list is a hypothesis), but the undisputed part needs no recording: AWS's ACME endpoint serves an HTTP ACME directory at the EndpointUrl it returns, answering 200 to GET <EndpointUrl> (RFC 8555 §7.1.1 directory resource, plus newNonce et al.).
Emulator: a plain GET of the URL DescribeAcmeEndpoint just returned answers HTTP 404 with body {"__type":"UnknownService","message":"no emulated service is registered for '4e3c8da6-227f-4044-b5e9-e18d3d41a993'"} — i.e. the emulator's front door parses the first host label (the endpoint uuid, see Cause 1) as a service name and finds no service. The emulator announces an address it does not serve.
Why: the entire ACME data plane is unreachable. An ACME client gets a 404 with a JSON error envelope where a directory object is required and cannot even fetch a nonce, so no account registration, order or issuance through an ACM ACME endpoint is possible.
Fix: route the generated ACME endpoint host/path to an ACME directory handler in the ACM service crate (the ACME HTTP surface), and make the front-door host router recognise the ACME enrollment host instead of treating its leading label as a service token. Same component as Cause 1 for the URL it must agree with.
Traces: acm.acme-directory-serves-the-rfc8555-resource-map-and-single-use-nonces.

## Evidence
Cause 1 (4 checks, identical assertion): AssertionError: EndpointUrl 'https://dde55c5b-2597-418a-a95b-d78c38505315.acme.us-west-2.amazonaws.com/directory' must be on acm-acme-enroll.us-west-2.<domain> (acm.acme-endpoint-directory-url-immutable-server-generated); same with 'https://da00a286-f680-409b-a748-59bc2dc5e420.acme...' (acm.acme-endpoint-directory-url-managed-by-service), 'https://4b5b9598-fe0d-45e5-bcf3-bbac6784f9c0.acme...' (acm.acme-endpoint-server-managed-directory-url) and 'https://a8184750-cbae-4646-9fab-4ca81b5c735c.acme...' (acm.describe-acme-endpoint-returns-server-managed-urls). The uuid differs per run and is not part of the requirement; the host form is.
Cause 2: AssertionError: the ACME directory at 'https://4e3c8da6-227f-4044-b5e9-e18d3d41a993.acme.us-west-2.amazonaws.com/directory' must be served; got HTTP 404 with body b'{"__type":"UnknownService","message":"no emulated service is registered for \'4e3c8da6-227f-4044-b5e9-e18d3d41a993\'"}' (acm.acme-directory-serves-the-rfc8555-resource-map-and-single-use-nonces).
Smithy citations: AcmeEndpoint{AcmeEndpointArn, EndpointUrl, Status, AuthorizationBehavior, Contact, CertificateAuthority, CertificateTags, CreatedAt, UpdatedAt}; CreateAcmeEndpointRequest{IdempotencyToken, AuthorizationBehavior, Contact, CertificateAuthority, Tags, CertificateTags} — no EndpointUrl member, so the URL is entirely server-managed.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. acm.create_acme_endpoint(AuthorizationBehavior=..., CertificateAuthority=...), poll acm.describe_acme_endpoint(AcmeEndpointArn=<arn>) until Status=ACTIVE.
3. Read AcmeEndpoint.EndpointUrl: it is 'https://<endpoint-uuid>.acme.<region>.amazonaws.com/directory' instead of a URL on the region's ACME enrollment host (acm-acme-enroll.<region>.<domain>).
4. GET that same EndpointUrl against the emulator: HTTP 404 with {"__type":"UnknownService"} instead of the ACME directory object (200).
5. Re-replay the five checks in trace_ids.
