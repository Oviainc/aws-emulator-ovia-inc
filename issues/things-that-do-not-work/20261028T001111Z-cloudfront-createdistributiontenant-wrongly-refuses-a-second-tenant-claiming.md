Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.domain-conflict-detection-flow
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistributionTenant|CNAMEAlreadyExists
Issue-Commit: 89b9ca4b685f6ae9a714db90fb8f95ad9a86cf49

# cloudfront CreateDistributionTenant wrongly refuses a second tenant claiming a domain that is only held by an unvalidated managed-certificate request (409 CNAMEAlreadyExists; AWS accepts with 201)

> _Note: this issue was identified against the code base at commit `89b9ca4b685f6ae9a714db90fb8f95ad9a86cf49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistributionTenant

## Description
This issue carries ONE net-new root cause in the cloudfront service.

Related open issue that does NOT cover this cause but belongs to the same family: 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md. Its cause 6 is the opposite direction of the same tenant-domain gap — there the emulator is too PERMISSIVE (a pending managed-certificate domain reads Status 'active' and a create AWS refuses is accepted). The cause below is the emulator being too STRICT on the same surface: it enforces a CNAME uniqueness constraint AWS does not apply to an unvalidated domain claim. Closing cause 6 of that issue does not close this one. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: CreateDistributionTenant enforces CNAME uniqueness against a domain that is claimed only by a tenant whose managed certificate request is still pending, so a duplicate claim AWS accepts is refused.

AWS: Smithy: CreateDistributionTenant (POST /2020-05-31/distribution-tenant, 201) models CNAMEAlreadyExists (409, documented "The CNAME specified is already defined for CloudFront.") alongside InvalidArgument (400) and InvalidAssociation (409). The model does not say WHEN a tenant domain becomes a CNAME claim. Recorded behavior shows the rule: a domain named by a tenant that holds only a pending ManagedCertificateRequest (ValidationTokenHost=self-hosted, the domain not yet validated) is NOT an enforced claim. In that state ListDomainConflicts itself refuses with 400 InvalidArgument and the message "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" — i.e. the service has no validated certificate to resolve the claim against — and a SECOND distribution tenant under the same multi-tenant (ConnectionMode=tenant-only) distribution naming the IDENTICAL domain is accepted with 201. Both tenants then read back independently via GetDistributionTenant, each listing that domain in its Domains. Only a VALIDATED domain claim produces CNAMEAlreadyExists. Note the emulator's refusal already satisfied the ListDomainConflicts half of this contract (that assertion passed), so the divergence is isolated to the create path.

Emulator: the second CreateDistributionTenant for the same domain is refused with 409 CNAMEAlreadyExists and the message "One or more of the CNAMEs you provided are already associated with a different resource: <domain>". Two defects in one: (a) the refusal should not happen at all for a domain whose only claimant has a pending certificate request, and (b) the message appends a request-derived value. AWS's text for this error class is the fixed sentence "One or more of the CNAMEs you provided are already associated with a different resource." — the whole sentence is fixed text and carries NOTHING from the request; the emulator appends ": <the Domains[0].Domain value from the request>", which is a value from the request and not part of the template. (This message-suffix habit is the same family as cause 10 of the related issue above, which covers a different set of error codes.)

Why: a client building a multi-tenant CloudFront estate cannot create the tenants it creates in production. The real workflow — stage several tenants against the same domain while the managed certificate validates, then let validation decide which claim wins — is unreachable because the emulator decides the conflict at create time, before any certificate exists. Code that treats CNAMEAlreadyExists as a real conflict (and gives up, or deletes the sibling tenant) takes a false branch offline, and the negative-class contract the check pins (unvalidated claims are not enforced) cannot be tested at all.

Fix: the domain/CNAME conflict check in CreateDistributionTenant's handler and the tenant-domain slice of state it consults — the claim index must only treat a domain as taken when its claimant's certificate is validated, and the CNAMEAlreadyExists message constructor must emit the fixed per-code sentence with no request-derived suffix.

Traces: cloudfront.domain-conflict-detection-flow.

## Evidence
Replay output, verbatim tail of cloudfront.domain-conflict-detection-flow ([error]):

  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.CNAMEAlreadyExists: An error occurred (CNAMEAlreadyExists) when calling the CreateDistributionTenant operation: One or more of the CNAMEs you provided are already associated with a different resource: sim2real-dcf-a6e60fd26c3e.com

The exception is raised from the check's second `_create_distribution_tenant(ctx, dist_id, [domain], enabled=False)` call (check source line 9623 of the report), i.e. the step annotated "An unvalidated domain claim is not enforced: a second tenant under the same multi-tenant distribution claiming the IDENTICAL domain is accepted."

Expected (recorded AWS): HTTP 201 with DistributionTenant.Id != the first tenant's Id and Domains == [{'Domain': <domain>}], then GetDistributionTenant on BOTH ids still listing <domain>.
Actual (emulator): HTTP 409, code CNAMEAlreadyExists, message "One or more of the CNAMEs you provided are already associated with a different resource: sim2real-dcf-a6e60fd26c3e.com".

Message breakdown: fixed text = "One or more of the CNAMEs you provided are already associated with a different resource." (the entire AWS sentence). Variable part added by the emulator and not present in AWS's message = the trailing ": <Domains[0].Domain from the request>" (here the generated domain sim2real-dcf-a6e60fd26c3e.com, a per-run value).

The earlier half of the same check PASSED against the emulator, which pins the state the create was made in: ListDomainConflicts(Domain=<domain>, DomainControlValidationResource={DistributionTenantId: <tenant1>}) returned 400 InvalidArgument with "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" — so the emulator itself agrees no validated certificate backs the claim, yet still enforces it as a CNAME conflict.

Smithy citation: CreateDistributionTenant -> POST /2020-05-31/distribution-tenant, success 201; errors AccessDenied(403), CNAMEAlreadyExists(409), EntityAlreadyExists(409), EntityLimitExceeded(400), EntityNotFound(404), InvalidArgument(400), InvalidAssociation(409), InvalidTagging(400). CNAMEAlreadyExists documentation: "The CNAME specified is already defined for CloudFront."

## Steps to Reproduce
1. CreateDistributionWithTags for a multi-tenant distribution: DistributionConfig with ConnectionMode='tenant-only', one CustomOriginConfig origin, DefaultCacheBehavior.CachePolicyId=658327ea-f89d-4fab-a63d-7e88639e58f6 (managed CachingOptimized). Keep the returned Distribution.Id.
2. Pick an unregistered domain D.
3. CreateDistributionTenant(DistributionId=<dist>, Name=<name1>, Domains=[{Domain: D}], Enabled=False, ManagedCertificateRequest={ValidationTokenHost: 'self-hosted', CertificateTransparencyLoggingPreference: 'disabled', PrimaryDomainName: D}) -> 201. GetDistributionTenant on it lists D.
4. ListDomainConflicts(Domain=D, DomainControlValidationResource={DistributionTenantId: <tenant1>}) -> 400 InvalidArgument, "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" (this already matches).
5. Repeat step 3 verbatim with a different Name and the SAME domain D.
   Expected (AWS): 201, a new tenant id, Domains carrying D; GetDistributionTenant on both tenant ids still lists D.
   Actual (emulator): 409 CNAMEAlreadyExists "One or more of the CNAMEs you provided are already associated with a different resource: <D>".
Or replay the check id in trace_ids.
