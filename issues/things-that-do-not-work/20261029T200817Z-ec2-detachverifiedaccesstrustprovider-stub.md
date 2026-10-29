Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-verified-access-delete-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DetachVerifiedAccessTrustProvider
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DetachVerifiedAccessTrustProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachVerifiedAccessTrustProvider

## Description
`DetachVerifiedAccessTrustProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_verified_access_trust_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DetachVerifiedAccessTrustProvider` call, so each one unblocks only when `DetachVerifiedAccessTrustProvider` works: `ec2-verified-access-delete-lifecycle#task#11`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_verified_access_trust_provider.rs`. Replaying the corpus, the emulator refused the `DetachVerifiedAccessTrustProvider` call: status: expected 200, got 501
$.DetachVerifiedAccessTrustProviderResponse: missing (expected {'requestId': '9957cbfb-202f-44de-8552-0a2020cbfb85', 'verifiedAccessTrustProvider': {'creationTime': '2026-10-29T19:59:32.12767Z', 'description': 'sim2real-probe-ec2-va-trust-56560216', 'lastUpdatedTime': '2026-09-02T00:15:05.000Z', 'nativeApplicationOidcOptions': {'authorizationEndpoint': 'https://example.com/oauth2/default/authorize', 'clientId': 'sim2real-probe-client-id', 'issuer': 'https://example.com/oauth2/default', 'publicSigningKeyEndpoint': 'https://example.com/oauth2/default/keys', 'scope': 'openid', 'tokenEndpoint': 'https://example.com/oauth2/default/token', 'userInfoEndpoint': 'https://example.com/oauth2/default/userinfo'}, 'oidcOptions': {'authorizationEndpoint': 'https://example.com/oauth2/default/authorize', 'clientId': 'sim2real-probe-client-id', 'clientSecret': 'REDACTED', 'issuer': 'https://example.com/oauth2/default', 'scope': 'openid', 'tokenEndpoint': 'https://example.com/oauth2/default/token', 'userInfoEndpoint': 'https://example.com/oauth2/default/userinfo'}, 'policyReferenceName': 'sim2rprobe', 'sseSpecification': {'customerManagedKeyEnabled': 'false'}, 'trustProviderType': 'user', 'userTrustProviderType': 'oidc', 'verifiedAccessTrustProviderId': 'vatp-0000000000000002e'}, 'verifiedAccessInstance': {'creationTime': '2026-10-29T19:59:32.072346Z', 'description': 'sim2real-probe-ec2-va-instance-56560216', 'fipsEnabled': 'false', 'lastUpdatedTime': '2026-09-02T00:15:05.000Z', 'verifiedAccessInstanceId': 'vai-0000000000000002d', 'verifiedAccessInstanceTransitGatewayAttachmentSet': '', 'verifiedAccessTrustProviderSet': ''}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DetachVerifiedAccessTrustProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
