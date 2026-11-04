Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.delete-domain-name-with-active-association-succeeds
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|DeleteDomainName|ConflictException
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# apigateway: REOPENS the DeleteDomainName fix — DeleteDomainName on a PRIVATE custom domain is refused 409 ConflictException because it has access associations

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomainName

## Description
REOPENS 20261025T172549Z-acm-apigateway-api-gateway-deletedomainname-is-still-an-unimplemented-501.md: its fix left the call refused.

Cause 1 of 1: DeleteDomainName now exists but refuses a PRIVATE custom domain that carries domain-name access associations.
AWS: DeleteDomainName on a PRIVATE custom domain name is not gated on that domain's domain-name access associations. The delete is accepted (Smithy: DeleteDomainName, DELETE /domainnames/{domainName}, success code 202; ConflictException httpError 409 is modeled but AWS does not raise it for this condition), the domain afterwards reads NotFoundException, and the access association is left behind as an orphan rather than refused or cascade-deleted.
Emulator: the call is refused HTTP 409 `ConflictException` with the message `The domain name <domainName> has domain name access associations. Delete the domain name access associations before deleting the domain name.` — a gate AWS does not apply to PRIVATE domains, so the domain can never be deleted while any association exists.
Why: a client tearing down a private custom domain is blocked on a precondition AWS does not impose. Its teardown ordering (delete the domain, then reap orphaned associations) deadlocks, and resources leak for the lifetime of the account; code that treats ConflictException here as a real conflict will retry forever.
Fix: the DeleteDomainName handler's precondition checks — for a PRIVATE endpoint-type domain, do not consult domain-name access associations at all; delete the domain, answer 202, leave the associations in place as orphans, and let subsequent reads of the domain answer NotFoundException.
Traces: apigateway.delete-domain-name-with-active-association-succeeds.

## Evidence
apigateway.delete-domain-name-with-active-association-succeeds ([error]):
`botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DeleteDomainName operation: The domain name <private-domain> has domain name access associations. Delete the domain name access associations before deleting the domain name.`

The check's own contract: "DeleteDomainName on a PRIVATE custom domain name is not gated on its domain-name access associations: the delete succeeds, the domain is gone (NotFoundException), and the association is left behind as an orphan rather than refused or cascade-deleted." Expected: HTTP 202 and no error; observed: HTTP 409 ConflictException.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay apigateway.delete-domain-name-with-active-association-succeeds.
3. Manual repro: CreateDomainName with endpointConfiguration.types=[PRIVATE]; CreateDomainNameAccessAssociation against that domain with accessAssociationSourceType=VPCE and a real VPC endpoint id; then call DeleteDomainName on the domain. Observed: 409 ConflictException "... has domain name access associations ...". Expected: 202, the domain subsequently NotFoundException, and the access association still listed as an orphan.
