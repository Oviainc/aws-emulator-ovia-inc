Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.authorization-token-custom-duration-sets-expiration codeartifact.authorization-token-default-duration-12h codeartifact.authorization-token-duration-shapes-expiry codeartifact.authorization-token-expires-and-is-rejected-after-ttl codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves codeartifact.codeartifact-npm-publish-and-install-through-repository-endpoint codeartifact.codeartifact-pypi-simple-index-serves-a-published-wheel codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository codeartifact.list-package-version-dependencies-preserves-declared-order codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|GetAuthorizationToken|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codeartifact GetAuthorizationToken: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAuthorizationToken

## Description
`GetAuthorizationToken` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_authorization_token.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `GetAuthorizationToken` call, so each one unblocks only when `GetAuthorizationToken` works: `codeartifact.authorization-token-custom-duration-sets-expiration`, `codeartifact.authorization-token-default-duration-12h`, `codeartifact.authorization-token-duration-shapes-expiry`, `codeartifact.authorization-token-expires-and-is-rejected-after-ttl`, `codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves`, `codeartifact.codeartifact-npm-publish-and-install-through-repository-endpoint`, `codeartifact.codeartifact-pypi-simple-index-serves-a-published-wheel`, `codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace`, `codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable`, `codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository`, `codeartifact.list-package-version-dependencies-preserves-declared-order`, `codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository`, `codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version`, `codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_authorization_token.rs`. Replaying the corpus, the emulator refused the `GetAuthorizationToken` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAuthorizationToken operation: operation GetAuthorizationToken is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeartifact emulator
Call GetAuthorizationToken and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
