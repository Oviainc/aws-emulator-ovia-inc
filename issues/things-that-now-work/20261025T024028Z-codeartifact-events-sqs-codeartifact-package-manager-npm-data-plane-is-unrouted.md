Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event
Issue-Key: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event|run#33|AssertionError|assert status == 200, f"npm publish into {upstream!r} returned {status!r}: {body[:300]!r}"
Issue-Commit: 9fe95bf5a6f42fe93a77d740b9cb51055f43a772

# CodeArtifact package-manager (npm) data plane is unrouted: the endpoint GetRepositoryEndpoint hands back answers 404 UnknownService, so no publish happens and no Package Version State Change event is emitted

> _Note: this issue was identified against the code base at commit `9fe95bf5a6f42fe93a77d740b9cb51055f43a772`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Cause 1 of 1 — the repository endpoint returned by GetRepositoryEndpoint is not served by the emulator, so every package-manager-protocol request against it is refused with a 404 UnknownService and the whole CodeArtifact data plane (and the eventing that depends on it) is unreachable.

AWS: Smithy `GetRepositoryEndpoint` (GET /v1/repository/endpoint; required members domain, repository, format; modeled errors AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400, ThrottlingException 429, InternalServerException 500) returns `repositoryEndpoint`, the URL of the repository's package-manager endpoint, of the form `https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/npm/<repository>/`. That endpoint is a live registry: with the bearer token from `GetAuthorizationToken`, an npm-protocol `PUT /<package>` publishes a version (HTTP 200) and `GET /<package>` returns the packument with a `dist.tarball` URL that itself serves the published bytes (HTTP 200). The npm registry wire protocol is not in the Smithy model — recorded behavior only: the recording shows the publish returning 200, DescribePackageVersion then reporting a revision for the version, and a `CodeArtifact Package Version State Change` event (source `aws.codeartifact`) reaching an EventBridge rule scoped to the domain.

Emulator: GetAuthorizationToken and GetRepositoryEndpoint now both succeed (the fix for the resolved 20261025T002357Z-codeartifact-events-sqs-codeartifact-getauthorizationtoken-is-an-unimplemented-scaffold-stub-so.md landed and the check advances past them), but the host named in the endpoint it returns is not registered with the emulator's request router. The npm `PUT` against that endpoint answers HTTP 404 with body `{"__type":"UnknownService","message":"no emulated service is registered for '<domain>-<domainOwner>'"}`. This is a different component and a different call from the already-resolved GetAuthorizationToken stub: the control-plane operation answers correctly and the refusal is now on the package-manager endpoint it advertises.

Why: a client is handed an endpoint URL by a 200 response and then cannot use it at all — every npm/pip/maven client flow (`npm publish`, `npm install`, upstream fetch and downstream retention) fails with a 404 that names no CodeArtifact resource and is not a modeled CodeArtifact error, so it cannot be classified or retried. Because nothing can be published, the downstream `CodeArtifact Package Version State Change` events for a publish and for an upstream retention are never produced either, so an EventBridge rule over CodeArtifact activity receives nothing. Note the advertised endpoint also embeds domainOwner `000000000000`; the host form is correct, only the routing is missing.

Fix: the emulator's endpoint/host router must serve the CodeArtifact package-manager data plane host it advertises (`<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com`, path `/npm/<repository>/...`) and dispatch npm-protocol PUT/GET (packument, tarball) to the CodeArtifact service, in services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice; a protocol publish must create the package version in CodeArtifact state (so DescribePackageVersion reports the revision it minted), honour upstream fetch/retention into a downstream repository, and emit the `CodeArtifact Package Version State Change` event to the default event bus. Related open CodeArtifact issue 20261024T061931Z-codeartifact-cfn-provisioned-domains-absent-resource-policies-never.md does NOT cover this: it concerns control-plane policy evaluation, CFN provisioning and refusal messages, and does not carry this check's replay key.

Traces: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event.

## Evidence
Replay of codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event:

  AssertionError: npm publish into 'sim2real-ca-up-46435183c3ea' returned 404: b'{"__type":"UnknownService","message":"no emulated service is registered for \'sim2real-ca-dom-649c9c92ae6c-000000000000\'"}'

Everything before it succeeded: CreateDomain, CreateRepository (upstream and a downstream with the upstream attached), PutRule + the SQS target (rule ENABLED, target list equal to the queue ARN), GetAuthorizationToken, and GetRepositoryEndpoint for both repositories. The failing request is the npm-protocol `PUT https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/npm/<upstream>/<package>` with the authorization token as bearer credential.

Expected: HTTP 200 for the publish; DescribePackageVersion then reporting a revision for <package>@1.0.0; one `CodeArtifact Package Version State Change` event (source `aws.codeartifact`, detail.domainName = the domain) delivered to the rule's queue. Observed: HTTP 404, `__type` UnknownService — a non-CodeArtifact error envelope from the router, so the publish, the retention leg, and both events never happen. (The host name, package names and account id above are per-run values and are examples; the requirement is that the advertised endpoint is served.)

## Steps to Reproduce
1. codeartifact.create_domain(domain=D); codeartifact.create_repository(domain=D, repository=U); codeartifact.create_repository(domain=D, repository=DN, upstreams=[{'repositoryName': U}]).
2. token = codeartifact.get_authorization_token(domain=D)['authorizationToken'] -> 200.
3. ep = codeartifact.get_repository_endpoint(domain=D, repository=U, format='npm')['repositoryEndpoint'] -> 200, e.g. https://<D>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<U>/
4. HTTP PUT ep + '<package>' with header 'Authorization: Bearer <token>' and an npm publish envelope (package.json metadata plus a base64 tarball attachment) -> observe HTTP 404 {"__type":"UnknownService", ...} instead of HTTP 200.
5. HTTP GET ep + '<package>' -> same 404; so the packument/tarball reads, the downstream upstream-fetch retention, and the two `CodeArtifact Package Version State Change` events to an EventBridge rule on detail.domainName = D never occur.
