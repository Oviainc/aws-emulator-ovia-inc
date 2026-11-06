Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.delete-domain-name-in-use-succeeds apigateway.delete-domain-name-with-active-basepath-mapping-conflict apigateway.delete-domain-name-with-active-mapping-cascades apigateway.delete-domain-name-with-active-mapping-succeeds apigateway.delete-domain-name-with-dependent-base-path-mapping-conflict
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|DeleteDomainName|BadRequestException
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# apigateway: REOPENS the DeleteDomainName fix — DeleteDomainName is refused 400 BadRequestException while a BasePathMapping names the domain, where AWS deletes the domain and cascades the mapping

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomainName

## Description
REOPENS 20261025T172549Z-acm-apigateway-api-gateway-deletedomainname-is-still-an-unimplemented-501.md: its fix left the call refused. DeleteDomainName no longer answers 501, but it now refuses the delete with a guard AWS does not apply, so a custom domain created with a base path mapping still cannot be deleted. This is not a new 501; it is the same operation, still unable to complete.

This issue carries ONE root cause (Cause 1 of 1), grounded in five failing checks none of which is marked ALREADY FILED or OWNED.

Cause 1 of 1: DeleteDomainName is gated on the absence of BasePathMappings.

AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) — DeleteDomainName: DELETE /domainnames/{domainName}, success 202, output Unit; modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). The model documents only "Deletes the DomainName resource" and says nothing that conditions the delete on dependent records, so for the dependency rule: recorded behavior only — real AWS DELETES a custom domain name that a BasePathMapping still references. The call succeeds (HTTP 202, empty body); afterwards GetDomainName for that name answers 404 NotFoundException with message "Invalid domain name identifier specified", and GetBasePathMapping for the mapping that referenced it answers the SAME 404 NotFoundException "Invalid domain name identifier specified" — the mapping is cascade-removed with the domain, not left dangling. ("Invalid domain name identifier specified" is fixed text with no variable part.) This holds for the default/empty base path and for a named base path such as "v1" alike. Note the contrast the suite pins elsewhere: DeleteRestApi and DeleteStage ARE refused while a BasePathMapping names them (see 20261105T160000Z-apigateway-base-path-mapping-guard-and-import-refusal-sentences.md) — the guard belongs on those two operations, not on DeleteDomainName.

Emulator: the call is refused with HTTP 400 BadRequestException and the message "The domain name <domainName> has base path mapping <basePath>. Delete the base path mappings before deleting the domain name." — <domainName> and <basePath> are taken from state, and an empty (default) base path is rendered as the literal "(none)", so even the default mapping created by CreateBasePathMapping with no basePath blocks the delete. The domain name survives the call. The guard is explicit in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_domain_name.rs lines 222-239 (`if let Some(base_path) = &base_path { return Err(... BadRequestException { message: crate::state::domain_name_has_base_path_mappings(...) }) }`), fed by `entry.base_paths_in_creation_order().first()` at line 196.

Why: a client that provisions a custom domain, maps a stage onto it, and then tears the stack down cannot delete the domain at all — teardown and CloudFormation delete/rollback paths that delete the DomainName before (or without) its mappings stall on a 400 AWS never emits. A client that relies on the documented cascade also never observes the mapping disappearing, and because the emulator's own sentence is not AWS's text, error handling keyed on the refusal wording sees a class of failure real AWS has no wording for. The account-wide ACM certificate the domain holds is never released either, so dependent cleanup fails behind it.

Fix: the DeleteDomainName handler in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_domain_name.rs — remove the base-path-mapping dependency guard and make the removal cascade: deleting the stored DomainName record must also drop the BasePathMapping records it holds, so a later GetDomainName and a later GetBasePathMapping both answer NotFoundException "Invalid domain name identifier specified". The sibling DomainNameAccessAssociation guard in the same handler is a separate question already tracked by 20261104T052719Z-apigateway-reopens-the-deletedomainname-fix-deletedomainname-on-a.md and is not what these five checks exercise.

Traces: apigateway.delete-domain-name-in-use-succeeds, apigateway.delete-domain-name-with-active-basepath-mapping-conflict, apigateway.delete-domain-name-with-active-mapping-cascades, apigateway.delete-domain-name-with-active-mapping-succeeds, apigateway.delete-domain-name-with-dependent-base-path-mapping-conflict.

## Evidence
All five checks are listed under the suite heading `### apigateway` and all five fail as [error] on the DeleteDomainName call itself (botocore raises before any assertion):

- apigateway.delete-domain-name-in-use-succeeds — `botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the DeleteDomainName operation: The domain name <domain>.example.com has base path mapping (none). Delete the base path mappings before deleting the domain name.` The check then cannot reach its two assertions: `get_domain_name` must answer NotFoundException / "Invalid domain name identifier specified", and `get_base_path_mapping(basePath="")` must answer the same.
- apigateway.delete-domain-name-with-active-mapping-succeeds — identical refusal, base path rendered "(none)".
- apigateway.delete-domain-name-with-active-basepath-mapping-conflict — `... has base path mapping v1. Delete the base path mappings before deleting the domain name.` after CreateBasePathMapping(basePath="v1"). The check's own docstring records the contract: "DeleteDomainName succeeds even while a base path mapping still references it (AWS does not gate domain deletion on the presence of base path mappings); the deletion removes both the domain name and the mapping that pointed at it."
- apigateway.delete-domain-name-with-active-mapping-cascades — same refusal with base path "v1".
- apigateway.delete-domain-name-with-dependent-base-path-mapping-conflict — same refusal with base path "v1".

Expected in every case: DeleteDomainName succeeds (HTTP 202), GetDomainName then answers 404 NotFoundException "Invalid domain name identifier specified", and GetBasePathMapping for the removed mapping answers 404 NotFoundException "Invalid domain name identifier specified".

The generated domain names, base path value "v1", api ids and certificate ARNs in the captures are this run's values filling the templates; the requirements are the 202 success, the cascade, and the two 404 messages.

## Steps to Reproduce
1. ACM ImportCertificate a self-signed certificate for <domain>.
2. apigateway CreateDomainName(domainName=<domain>, regionalCertificateArn=<cert>, endpointConfiguration={types:[REGIONAL]}) and poll GetDomainName until domainNameStatus is AVAILABLE.
3. CreateRestApi; PutMethod GET (authorizationType=NONE) + MOCK integration on the root resource; CreateDeployment(stageName="prod").
4. CreateBasePathMapping(domainName=<domain>, restApiId=<id>, stage="prod") — once with basePath="v1" and once with basePath omitted (the default/empty base path).
5. DeleteDomainName(domainName=<domain>). Observed: HTTP 400 BadRequestException "The domain name <domain> has base path mapping v1. Delete the base path mappings before deleting the domain name." (or "... base path mapping (none). ..." for the default mapping), and the domain survives. Expected: HTTP 202 success.
6. GetDomainName(domainName=<domain>) and GetBasePathMapping(domainName=<domain>, basePath=<the mapping>): both must answer 404 NotFoundException with message "Invalid domain name identifier specified".
7. Re-replay the five check ids in trace_ids to confirm.
