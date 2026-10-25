Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event
Issue-Key: codeartifact__events__sqs|GetAuthorizationToken|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# CodeArtifact GetAuthorizationToken is an unimplemented scaffold stub, so no package can be published and no Package Version State Change event can be emitted (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAuthorizationToken

## Description
Cause 1 of 1 — CodeArtifact `GetAuthorizationToken` answers NotImplemented.
AWS: `GetAuthorizationToken` takes a domain (and optional domainOwner/durationSeconds) and returns `authorizationToken` plus `expiration`; the token is the bearer credential every package-manager client and every repository write uses. Modeled client errors are AccessDeniedException, ResourceNotFoundException, ValidationException, ThrottlingException and InternalServerException.
Emulator: answers HTTP 400 with error code `NotImplemented` and the message "operation GetAuthorizationToken is scaffolded but not implemented in this emulator". `NotImplemented` is not a modeled error for this operation.
Why: the token is the entry point to the whole CodeArtifact data plane, so nothing can be published to or read from a repository. Downstream, the "CodeArtifact Package Version State Change" events that a publish and an upstream-retention fetch each emit to EventBridge can never be produced, so a rule over CodeArtifact activity receives nothing — and the client sees a non-modeled error code it cannot classify or retry on.
Fix: the GetAuthorizationToken handler, services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_authorization_token.rs.
Traces: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event.

## Evidence
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAuthorizationToken operation: operation GetAuthorizationToken is scaffolded but not implemented in this emulator". The check had already created the domain, the repositories and the EventBridge rule and queue successfully; it fails at the first data-plane step. Expected: HTTP 200 with `authorizationToken` and `expiration`.

## Steps to Reproduce
1. CreateDomain(domain=d); CreateRepository(domain=d, repository=r).
2. GetAuthorizationToken(domain=d) -> HTTP 400, code NotImplemented, instead of 200 with authorizationToken and expiration.
