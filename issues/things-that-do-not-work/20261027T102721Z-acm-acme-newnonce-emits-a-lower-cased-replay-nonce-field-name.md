Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.update-acme-endpoint-on-revoked-account-succeeds
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.update-acme-endpoint-on-revoked-account-succeeds|_acme_register_account#12|AssertionError|assert nonce, f"no Replay-Nonce returned from newNonce (status={status})"
Issue-Commit: c048498fa9bf8a008294e81bd378b5619adc830b

# ACM ACME newNonce emits its nonce field lower-cased on the wire, so a client that reads the field name as received never gets a nonce (1 cause)

This is cause 3 of
`20261027T102721Z-acm-acme-external-account-binding-credential-surface-diverges.md`,
split out and left OPEN when that issue's other two causes were fixed: those were two
control-plane gaps in `GetAcmeExternalAccountBindingCredentials` (a cross-account ARN
classified as a lookup miss, and a missing identity-based authorization layer), both
fixable inside the service crate, while this one is decided by the response-writing seam
BELOW every service crate (see the known blocker). Its one check still fails.

## Operation
The ACME data plane's `newNonce` resource (not a modelled Smithy operation)

## Description
Cause 1 of 1: the `newNonce` answer's nonce field reaches the client spelled `replay-nonce`, not the `Replay-Nonce` RFC 8555 §7.2 fixes and AWS's ACME endpoints put on the wire.
AWS: recorded behavior only (the ACME data plane is not a modelled Smithy operation). RFC 8555 §7.2 spells the field `Replay-Nonce` and requires it on both the 200 answer to `HEAD <newNonce>` and the 204 answer to `GET <newNonce>`; AWS's ACM ACME endpoints serve it under that exact spelling, which is why an ACME client that reads the response's field names as received finds it.
Emulator: the statuses are correct (200 for HEAD, 204 for GET), the nonce is fresh per request, and `Cache-Control: no-store` and the `Link` index are present — only the field NAME differs: it arrives lower-cased, so a client matching the canonical spelling is handed nothing and cannot sign a single JWS. `services/service-com-amazonaws-acm-certificatemanager/src/service/acme_http.rs` (`REPLAY_NONCE_FIELD`, `fn nonce`) already STATES the field under its canonical name; the casing is lost after that (see the known blocker).
Why: an ACME client (certbot, lego, acme.sh) takes its first nonce from `newNonce` and can send no JWS without it. Against AWS the client proceeds; against the emulator it wedges before the first POST, so every downstream ACME contract is unreachable for a reason unrelated to the implementation behind it.
Fix: whatever writes the response's field names has to preserve the canonical spelling for this surface; nothing inside `services/` reaches those bytes.
Traces: acm.update-acme-endpoint-on-revoked-account-succeeds.

## Evidence
acm.update-acme-endpoint-on-revoked-account-succeeds [mismatch]:
  AssertionError: no Replay-Nonce returned from newNonce (status=204), raised from the check's `_acme_register_account` after it did `HEAD <directory['newNonce']>` and then, finding no field named `Replay-Nonce`, retried with `GET <newNonce>` and got HTTP 204 with still no field under that name.
  Upstream all passed: CreateAcmeEndpoint -> ACTIVE, the external account binding created, GetAcmeExternalAccountBindingCredentials returned KeyId/MacKey, DescribeAcmeEndpoint returned EndpointUrl, and `GET <EndpointUrl>` served the directory 200 with newNonce/newAccount URLs.
  Unreached after that point: ACME newAccount registration, DescribeAcmeAccount showing VALID, RevokeAcmeAccount, UpdateAcmeEndpoint(Contact=...) succeeding on an endpoint whose only ACME account is REVOKED while the account stays REVOKED, and the fresh-endpoint positive control.
  Distinguishable from the already-filed JWS gap (`20261027T001842Z-acm-acme-data-plane-newaccount-answers-http-501.md`, whose evidence records "HEAD newNonce (Replay-Nonce present)" because the checks behind it normalise field names to lower case before reading them): this check never reaches a POST, so that issue's fix does not close it.

## Steps to Reproduce
1. Boot the emulator, create an ACME endpoint, poll DescribeAcmeEndpoint until Status == ACTIVE and read AcmeEndpoint.EndpointUrl.
2. `GET <EndpointUrl>` — the directory is served 200; take `directory['newNonce']`.
3. `HEAD <newNonce>` and read the response's field names exactly as received, without lower-casing them. No field named `Replay-Nonce` is present.
4. `GET <newNonce>` — HTTP 204, again no field under that name; the nonce is present only under `replay-nonce`.
5. AWS serves the field as `Replay-Nonce` on both the HEAD 200 and the GET 204, so a client there has a nonce and proceeds.

## Known blocker
The field name is normalised to lower case TWICE after a handler states it, both times
outside `services/`, which is the only tree a fixer may change:

* `aws_smithy_runtime_api::http::Headers` stores a field name as an `http::HeaderName`,
  which lower-cases on construction — so the name is already normalised inside the
  response object the ACME surface builds;
* `vendor/smithy-server-runtime/src/multi_service_router.rs:411-430`
  (`http_response_from_runtime_api`) rebuilds the outgoing response name by name through
  `http::Response::builder().header(name, value)`, parsing each name into an
  `http::HeaderName` again;
* the HTTP/1 writer (`axum_server`/`hyper`, reached at `vendor/server/src/lib.rs:1205`)
  then writes the `HeaderMap`'s stored names; hyper's title-case option for that encoder
  is not exposed by this listener.

So no edit inside `services/` can change the bytes a client receives — which is also why
no test in the service crate can observe the casing: the type a handler writes into has
already normalised it. Closing this needs a read-only `vendor/` change: a response seam
that preserves a field name's canonical spelling for a surface that asks for it (or the
listener's title-case option), after which the ACME surface needs no change at all
because it already states the canonical name. A fixer picking this up should either land
that core change or close it with mark_wont_fix(vendor_blocker) naming the two vendor
files above.
