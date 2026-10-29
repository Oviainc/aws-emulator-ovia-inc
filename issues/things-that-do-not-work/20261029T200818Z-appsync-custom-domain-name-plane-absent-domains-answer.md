Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-domain-association-roundtrip
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-domain-association-roundtrip appsync-domain-name-error-paths
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|AssociateApi|4xx->4xx|$.message
Issue-Key: appsync|AssociateApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|AssociateApi|4xx->4xx|status
Issue-Key: appsync|CreateDomainName|4xx->2xx|$.message
Issue-Key: appsync|CreateDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateDomainName|4xx->2xx|status
Issue-Key: appsync|GetDomainName|4xx->2xx|$.message
Issue-Key: appsync|GetDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|GetDomainName|4xx->2xx|status
Issue-Key: appsync|GetApiAssociation|4xx->4xx|$.message
Issue-Key: appsync|AssociateApi|4xx->2xx|$.message
Issue-Key: appsync|AssociateApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|AssociateApi|4xx->2xx|status
Issue-Key: appsync|DisassociateApi|4xx->4xx|$.message
Issue-Key: appsync|DisassociateApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|DisassociateApi|4xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync custom-domain-name plane: absent domains answer 200, and refusals carry the wrong code and sentence (6 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateApi

## Description
The custom-domain-name surface has no existence or certificate gate: operations addressed at a domain that was never created answer 200, CreateDomainName accepts a certificate in the wrong region, and the refusals that do fire carry the wrong error class or the wrong sentence. 6 net-new causes in the domain-name handlers and their state.

Cause 1 of 6 — AssociateApi with an unknown apiId answers NotFoundException about the domain instead of BadRequestException about the API.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, message template "Invalid API ID <apiId>" — fixed text "Invalid API ID " plus the apiId from the request body (recorded behavior; Smithy: AssociateApi errors are AccessDeniedException(403), BadRequestException(400), InternalFailureException(500), NotFoundException(404); input members domainName @httpLabel and apiId @required).
Emulator: 404 NotFoundException 'Domain name not found.' — it refuses on the domain it was able to resolve rather than on the API it could not.
Why: the diagnostic blames the wrong input, so a client re-creates a domain that already exists instead of fixing the apiId.
Fix: AssociateApi validation — resolve and validate apiId, and order that check so a valid domain is not reported as missing.
Traces: appsync-domain-association-roundtrip#task#5.

Cause 2 of 6 — CreateDomainName does not refuse a certificate outside us-east-1.
AWS: 400 BadRequestException with the fixed sentence "Certificate must be in us-east-1." — AppSync custom domain names require an ACM certificate in us-east-1 (recorded behavior; Smithy: CreateDomainName errors include BadRequestException httpError 400).
Emulator: 200, no error body and no x-amzn-errortype — the domain name is created.
Why: a client gets a domain name that cannot exist on AWS, and the misconfiguration is only discovered at migration.
Fix: CreateDomainName validation — check the certificateArn's region before creating.
Traces: appsync-domain-name-error-paths#task#3.

Cause 3 of 6 — GetDomainName for a domain that was never created answers 200.
AWS: 404 NotFoundException with the fixed sentence "Domain name not found" (recorded behavior; Smithy: GetDomainName errors include NotFoundException httpError 404).
Emulator: 200, with no message and no x-amzn-errortype — it serves a domainNameConfig for a domain that does not exist.
Why: existence probes always succeed, so a client's create-if-absent logic never creates, and it then operates on a phantom domain.
Fix: GetDomainName handler — look the domain up in state and refuse when absent.
Traces: appsync-domain-name-error-paths#task#4.

Cause 4 of 6 — GetApiAssociation for an unknown domain refuses on the association rather than the domain.
AWS: 404 NotFoundException with the fixed sentence "Domain name not found" — AWS resolves the domain first (recorded behavior).
Emulator: 404 'API association not found.'
Why: a client cannot tell a missing domain from a domain with no association, which are different remedies.
Fix: GetApiAssociation handler — resolve the domain before the association, and use the domain's refusal sentence.
Traces: appsync-domain-name-error-paths#task#6.

Cause 5 of 6 — AssociateApi against a domain that was never created answers 200.
AWS: 404 NotFoundException with the fixed sentence "Domain name not found." (recorded behavior).
Emulator: 200, no message, no errortype — the association is created against a nonexistent domain.
Why: the emulator fabricates associations AWS rejects, so the whole domain-association flow proceeds on state that cannot exist.
Fix: AssociateApi handler — require the domain to exist in state.
Traces: appsync-domain-name-error-paths#task#7.

Cause 6 of 6 — DisassociateApi for a domain that was never created answers ConcurrentModificationException instead of NotFoundException.
AWS: 404 NotFoundException with the fixed sentence "Domain name not found." (recorded behavior; Smithy: DisassociateApi errors include NotFoundException httpError 404 and ConcurrentModificationException httpError 409).
Emulator: 409 with x-amzn-errortype ConcurrentModificationException and 'Another modification is in progress at this time and it must complete before you can make your change.'
Why: 409/ConcurrentModification is a retryable signal, so a client retries indefinitely against a domain that will never exist.
Fix: DisassociateApi handler — the existence check must precede the concurrency gate.
Traces: appsync-domain-name-error-paths#task#8.

Related open issues that do NOT cover these: "AppSync custom domain names: CreateDomainName performs no certificate validation (accepts a cert AWS refuses ... 'Certificate is ...')" (20261027T212657Z), "AppSync CreateDomainName answers 501 NotImplemented instead of rejecting an untrusted (private-CA-chain) certificate" (20261025T182150Z) and "AppSync Event-API/domain control plane: ... AssociateApi-GetApiAssociation-DisassociateApi ..." (20261025T041655Z) — none of them carries these replay keys, and cause 2 here is the certificate REGION rule, not the trust-chain rule.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-domain-association-roundtrip#task#5 AssociateApi: status expected 400 got 404; $.message expected 'Invalid API ID nonexistent-api-id-000000' got 'Domain name not found.'; x-amzn-errortype expected 'BadRequestException' got 'NotFoundException'.
2) appsync-domain-name-error-paths#task#3 CreateDomainName: status expected 400 got 200; $.message expected 'Certificate must be in us-east-1.' got None; errortype header missing.
3) appsync-domain-name-error-paths#task#4 GetDomainName: status expected 404 got 200; $.message expected 'Domain name not found' got None.
4) appsync-domain-name-error-paths#task#6 GetApiAssociation 404: $.message expected 'Domain name not found' got 'API association not found.'
5) appsync-domain-name-error-paths#task#7 AssociateApi: status expected 404 got 200; $.message expected 'Domain name not found.' got None.
6) appsync-domain-name-error-paths#task#8 DisassociateApi: status expected 404 got 409; $.message expected 'Domain name not found.' got 'Another modification is in progress at this time and it must complete before you can make your change.'; errortype got 'ConcurrentModificationException'.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. Fixtures appsync-domain-association-roundtrip and appsync-domain-name-error-paths mismatch at the steps under Traces.
3. By hand: CreateDomainName for a domain with a certificateArn in a region other than us-east-1 (cause 2); AssociateApi on an existing domain with an apiId that does not exist (cause 1); then, for a domain name that was never created, call GetDomainName (cause 3), GetApiAssociation (cause 4), AssociateApi (cause 5) and DisassociateApi (cause 6).
