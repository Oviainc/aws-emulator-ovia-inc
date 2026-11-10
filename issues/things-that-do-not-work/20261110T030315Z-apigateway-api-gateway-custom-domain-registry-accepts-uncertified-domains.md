Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-custom-domain-and-base-path-mapping-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-custom-domain-and-base-path-mapping-error-paths apigateway-domain-name-basepath-negative-paths
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateDomainName|4xx->2xx|$.regionalDomainName
Issue-Key: apigateway|GetDomainName|4xx->2xx|$.regionalDomainName
Issue-Key: apigateway|UpdateDomainName|4xx->2xx|$.regionalDomainName
Issue-Related: 20261029T200808Z-apigateway-api-gateway-custom-domain-names-and-access-associations.md
Issue-Commit: c01090bc0e97483ffdf7a7efd2d0a67727486686

# API Gateway custom domain registry accepts uncertified domains and resolves unknown ones

> _Note: this issue was identified against the code base at commit `c01090bc0e97483ffdf7a7efd2d0a67727486686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
3 causes. The custom-domain-name registry admits a domain AWS would have refused, and then every read/write of that non-existent domain succeeds instead of reporting it missing; each 2xx body also advertises a regionalDomainName under execute-api.<region>.amazonaws.com that this emulator does not serve.

Cause 1 of 3 - CreateDomainName with endpointConfiguration.types=[REGIONAL] and no certificate (no regionalCertificateArn, no certificateArn, no certificateBody/PrivateKey).
AWS: 400 BadRequestException, message 'A certificate was not provided for the endpoint type REGIONAL.' - the whole sentence is fixed text except the endpoint type, which is the request's endpointConfiguration type (REGIONAL / EDGE / PRIVATE). Smithy: CreateDomainName POST /domainnames (success 201), input member domainName @required, certificateArn/regionalCertificateArn optional, errors BadRequestException @httpError 400, ConflictException 409, LimitExceededException 429, UnauthorizedException 401. The 400 status and the message text are taken from the recorded response.
Emulator: 201 with a full DomainName representation, so the domain is created; the response also carries regionalDomainName 'd-<id>.execute-api.<region>.amazonaws.com', a host the emulator is not reached on (replay verdict host_not_own), and omits the x-amzn-errortype header entirely.
Why: a client's negative test for a missing certificate passes, and the account is left holding a domain that could never exist on AWS; any client that follows regionalDomainName to wire up DNS or a base-path mapping is sent to an address nothing answers on.
Fix: CreateDomainName's input validation (refuse a REGIONAL/EDGE domain with no certificate before touching state), and the handler's regionalDomainName synthesis, which must derive the host from the endpoint the emulator serves rather than a hard-coded execute-api domain.
Traces: apigateway-custom-domain-and-base-path-mapping-error-paths#task#3 (2 steps in the cluster).

Cause 2 of 3 - GetDomainName for a domain name that was never created.
AWS: 404 NotFoundException, message 'Invalid domain name identifier specified' - fixed text, no value interpolated; x-amzn-errortype present. Smithy: GetDomainName GET /domainnames/{domainName}, errors NotFoundException @httpError 404, BadRequestException 400.
Emulator: 200 with a DomainName body (including the unservable regionalDomainName) and no message / no x-amzn-errortype.
Why: a client cannot distinguish an existing custom domain from a missing one, so 'does this domain exist' probes and idempotent create-if-absent logic take the wrong branch.
Fix: GetDomainName's lookup must return NotFoundException when the domain name is absent from state.
Traces: apigateway-custom-domain-and-base-path-mapping-error-paths#task#4.

Cause 3 of 3 - UpdateDomainName (PATCH) against a domain name that was never created.
AWS: 404 NotFoundException, message 'Invalid domain name identifier specified' - fixed text; x-amzn-errortype present. Smithy: UpdateDomainName PATCH /domainnames/{domainName}, errors NotFoundException 404, BadRequestException 400, ConflictException 409.
Emulator: 200 with a DomainName body (again including the unservable regionalDomainName), no message and no x-amzn-errortype, i.e. the patch is reported as applied to a domain that does not exist.
Why: a client's update of a mistyped or deleted domain silently 'succeeds' and the client believes a configuration change landed that was never stored.
Fix: UpdateDomainName must resolve the domain before applying patchOperations and raise NotFoundException otherwise.
Traces: apigateway-custom-domain-and-base-path-mapping-error-paths#task#5.

The open issue '20261029T200808Z-apigateway-api-gateway-custom-domain-names-and-access-associations.md' describes neighbouring domain-name refusals (and owns the DeleteDomainName cluster of this same fixture), but the replay reported no owner for these three clusters - their host_not_own regionalDomainName key is outside that issue's replay key - so they are filed here.

## Evidence
Cause 1 (CreateDomainName, apigateway-custom-domain-and-base-path-mapping-error-paths#task#3): expected status 400 with $.message 'A certificate was not provided for the endpoint type REGIONAL.' and $headers.x-amzn-errortype present; got 201, $.message absent, no x-amzn-errortype, and $.regionalDomainName set to an execute-api host the emulator does not serve (verdict host_not_own).
Cause 2 (GetDomainName, #task#4): expected status 404 with $.message 'Invalid domain name identifier specified' and x-amzn-errortype present; got 200 with a DomainName body, $.message missing, $.regionalDomainName host_not_own.
Cause 3 (UpdateDomainName, #task#5): expected status 404 with $.message 'Invalid domain name identifier specified' and x-amzn-errortype present; got 200 with a DomainName body, $.message missing, $.regionalDomainName host_not_own.
Fixed vs variable text: 'A certificate was not provided for the endpoint type ' + <endpoint type from the request> + '.'; 'Invalid domain name identifier specified' is wholly fixed. The 'd-<id>.execute-api.<region>.amazonaws.com' value itself is per-run and is not the requirement - serving a host the emulator does not answer on is.

## Steps to Reproduce
1. Replay the corpus: replay_wire_corpus().
2. Cause 1: POST /domainnames with a fresh domainName and endpointConfiguration.types=[REGIONAL], supplying no certificateArn/regionalCertificateArn (fixture apigateway-custom-domain-and-base-path-mapping-error-paths, step task#3). Expect 400 BadRequestException 'A certificate was not provided for the endpoint type REGIONAL.'; observe 201.
3. Cause 2: GET /domainnames/<that same never-created domain> (same fixture, step task#4). Expect 404 'Invalid domain name identifier specified'; observe 200.
4. Cause 3: PATCH /domainnames/<that domain> with any patchOperations (same fixture, step task#5). Expect 404 'Invalid domain name identifier specified'; observe 200.
