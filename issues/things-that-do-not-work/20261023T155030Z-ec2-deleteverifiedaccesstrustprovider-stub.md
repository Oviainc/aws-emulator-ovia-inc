Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-verified-access-delete-lifecycle
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteVerifiedAccessTrustProvider
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# Implement ec2 DeleteVerifiedAccessTrustProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVerifiedAccessTrustProvider

## Description
`DeleteVerifiedAccessTrustProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_trust_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVerifiedAccessTrustProvider` call, so each one unblocks only when `DeleteVerifiedAccessTrustProvider` works: `ec2-verified-access-delete-lifecycle#task#12`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_trust_provider.rs`. Replaying the corpus, the emulator refused the `DeleteVerifiedAccessTrustProvider` call: status: expected 200, got 501
$.DeleteVerifiedAccessTrustProviderResponse: missing (expected {'requestId': '1da91cec-ab1d-46a6-9593-193c63b170e6', 'verifiedAccessTrustProvider': {'creationTime': '2026-10-23T15:43:12.234439Z', 'description': 'sim2real-probe-ec2-va-trust-56560216', 'lastUpdatedTime': '2026-09-02T00:15:10.000Z', 'nativeApplicationOidcOptions': {'authorizationEndpoint': 'https://example.com/oauth2/default/authorize', 'clientId': 'sim2real-probe-client-id', 'issuer': 'https://example.com/oauth2/default', 'publicSigningKeyEndpoint': 'https://example.com/oauth2/default/keys', 'scope': 'openid', 'tokenEndpoint': 'https://example.com/oauth2/default/token', 'userInfoEndpoint': 'https://example.com/oauth2/default/userinfo'}, 'oidcOptions': {'authorizationEndpoint': 'https://example.com/oauth2/default/authorize', 'clientId': 'sim2real-probe-client-id', 'clientSecret': 'REDACTED', 'issuer': 'https://example.com/oauth2/default', 'scope': 'openid', 'tokenEndpoint': 'https://example.com/oauth2/default/token', 'userInfoEndpoint': 'https://example.com/oauth2/default/userinfo'}, 'policyReferenceName': 'sim2rprobe', 'sseSpecification': {'customerManagedKeyEnabled': 'false'}, 'trustProviderType': 'user', 'userTrustProviderType': 'oidc', 'verifiedAccessTrustProviderId': 'vatp-00000000000000022'}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteVerifiedAccessTrustProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
