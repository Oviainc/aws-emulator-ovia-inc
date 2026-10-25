Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.domain-reassociation-conflict-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.domain-reassociation-conflict-rejected|run#11|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 39e54f9f86d0b102f322a2103cd9927458aab9c3

# AppSync CreateDomainName answers 501 NotImplemented instead of rejecting an untrusted (private-CA-chain) certificate with BadRequestException "Certificate is invalid."

> _Note: this issue was identified against the code base at commit `39e54f9f86d0b102f322a2103cd9927458aab9c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
1 cause in this issue; it must be closed before submitting.

=== Cause 1 of 1: CreateDomainName never performs certificate validation because the operation is a scaffold stub, so its documented BadRequestException refusal (and the companion GetDomainName NotFoundException) can never be produced ===

AWS: Smithy (appsync, com.amazonaws.appsync#AWSDeepdishControlPlaneService) models CreateDomainName as POST /v1/domainnames with required members `domainName` (DomainName, @length 1..253, @pattern ^(\*[\w\d-]*\.)?([\w\d-]+\.)+[\w\d-]+$) and `certificateArn` (CertificateArn, @length 20..2048, @pattern ^arn:[a-z-]*:(acm|iam):[a-z0-9-]*:\d{12}:(certificate|server-certificate)/[0-9A-Za-z_/-]*$), returning `domainNameConfig`, with the error set { AccessDeniedException 403, BadRequestException 400, InternalFailureException 500 }. `NotImplemented` is not in that set. AppSync validates the referenced certificate at CreateDomainName time and permanently refuses one whose trust chain is not publicly trusted (a locally generated private CA): the recorded real-AWS response is HTTP 400, error code `BadRequestException`, message `Certificate is invalid.` — that message is entirely fixed text for this error class, with no request- or state-derived parts. The recording establishes the refusal is terminal, not a propagation window (retried across cert-propagation windows up to ~500s with no change in outcome; corroborated by a second corpus recording, check_domain_name_crud_lifecycle.gt.jsonl, failing identically at ~90s). Because the create is refused, no domain resource exists afterwards, so GetDomainName on that domainName answers its modeled NotFoundException (404) with message `Domain name not found` — fixed text; the domainName that was asked for is the request-derived part of the request, not of the message.

Emulator: CreateDomainName answers the scaffold response — error code `NotImplemented` with message "operation CreateDomainName is scaffolded but not implemented in this emulator" — for every request, valid or invalid. The check's assertion on the refusal fails with `AssertionError: expected BadRequestException creating a domain name backed by a private-CA-chain certificate, got NotImplemented`. The positive control in the same check passes (CreateGraphqlApi + ListGraphqlApis both succeed), so this is specific to the domain-name operation and not a blanket credential/account failure. GetDomainName is likewise a stub, so the follow-on "nothing was created" probe cannot return NotFoundException/"Domain name not found" either.

Why: a client cannot distinguish "your certificate is unusable" from "this API does not exist". `NotImplemented` is in no SDK's modeled error set for CreateDomainName, so `except appsync.exceptions.BadRequestException` never catches the refusal and it escapes as a generic ClientError; code that branches on the error code to tell a permanent certificate-trust failure from a retryable condition takes the wrong branch and will retry forever. The forbidden-effect assertion is also unverifiable: a caller cannot confirm via GetDomainName that the rejected create left no domain resource behind, because that read answers NotImplemented rather than NotFoundException.

Fix: the CreateDomainName handler in the AppSync service crate (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_domain_name.rs) plus its certificate-trust validation, its domain-name state, and the GetDomainName read path that must then report NotFoundException for a domain that was never created. Related open issues name the same stubs but do NOT carry this check: 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md (its Cause 1 covers the domain-name operations being stubs, grounded only in the access-denied/list/get checks) and 20261024T032443Z-appsync-acm-importcertificate-is-an-unimplemented-scaffold-stub-blocking.md (the ACM side). Neither states the certificate-trust refusal contract (BadRequestException / "Certificate is invalid.") that this check asserts, so closing them does not necessarily close this.

Traces: appsync.domain-reassociation-conflict-rejected.

## Evidence
Expected (real-AWS recording): CreateDomainName(domainName=<generated>.sim2real-appsync-test.com, certificateArn=<ARN of a cert chained to a locally generated private CA>) -> HTTP 400, Error.Code `BadRequestException`, Error.Message `Certificate is invalid.`; then GetDomainName(domainName=<same>) -> HTTP 404, Error.Code `NotFoundException`, Error.Message `Domain name not found`.

Actual (emulator): CreateDomainName -> Error.Code `NotImplemented`, message "operation CreateDomainName is scaffolded but not implemented in this emulator". Replay output for appsync.domain-reassociation-conflict-rejected:

  File "check_appsync_domain_reassociation_conflict_rejected.py", line 55, in run
    assert code == "BadRequestException", (
  AssertionError: expected BadRequestException creating a domain name backed by a private-CA-chain certificate, got NotImplemented

Positive control in the same check succeeded immediately before the failing call: CreateGraphqlApi(authenticationType=API_KEY) returned an apiId and ListGraphqlApis contained it — so AppSync and the credentials are healthy and the NotImplemented is the operation's own scaffold, not an authorization or account problem. The fixed text of each message is quoted above in full; the only request-derived value involved is the `domainName`/`certificateArn` that are sent, which do not appear in either message.

## Steps to Reproduce
1. Create an AppSync GraphQL API as a positive control: appsync.create_graphql_api(name=<unique>, authenticationType="API_KEY") and confirm it appears in appsync.list_graphql_apis() — it does.
2. Mint a server certificate for a throwaway domain (e.g. <unique>.sim2real-appsync-test.com) whose trust chain is a locally generated private CA (openssl), and import it so it has an ACM/IAM certificate ARN.
3. Call appsync.create_domain_name(domainName=<that domain>, certificateArn=<that ARN>).
   Expected: HTTP 400 BadRequestException with message "Certificate is invalid." (terminal — unchanged across retries).
   Actual: error code NotImplemented ("operation CreateDomainName is scaffolded but not implemented in this emulator").
4. Call appsync.get_domain_name(domainName=<that domain>).
   Expected: HTTP 404 NotFoundException with message "Domain name not found", proving the rejected create left no resource behind.
   Actual: NotImplemented.
5. Replay appsync.domain-reassociation-conflict-rejected from the behavioral suite to confirm.
