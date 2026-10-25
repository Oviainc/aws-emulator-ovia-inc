Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.managed-certificate-status-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.managed-certificate-status-lifecycle-transitions|_client_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# CloudFront CreateDistributionTenant accepts a ManagedCertificateRequest with ValidationTokenHost=cloudfront for a domain whose ownership cannot be verified (1 cause)

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistributionTenant

## Description
One net-new root cause in the cloudfront service. Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUE (does NOT cover this cause): 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, cause 6 of 10, covers a neighbouring rule in the same family — that a tenant domain whose managed certificate is still pending reads Status "active" instead of "inactive". That half now behaves: in this run the self-hosted leg of the check succeeded and GetManagedCertificateDetails did report CertificateStatus "pending-validation". What remains unimplemented is the DIFFERENT, synchronous admission rule described below, which is keyed on ValidationTokenHost, not on the domain's status representation. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

=== Cause 1 of 1: ValidationTokenHost="cloudfront" does not demand verifiable domain ownership ===
AWS: Smithy: CreateDistributionTenant (POST /2020-05-31/distribution-tenant, success 201) models InvalidArgument (400) among AccessDenied (403), CNAMEAlreadyExists (409), EntityAlreadyExists (409), EntityLimitExceeded (400), EntityNotFound (404), InvalidAssociation (409), InvalidTagging (400). Its ManagedCertificateRequest member carries the required enum ValidationTokenHost with exactly two values, `cloudfront` and `self-hosted`; the model documents the distinction — for `cloudfront` "CloudFront will automatically serve the validation token. Choose this mode if you can point the domain's DNS to CloudFront immediately", whereas `self-hosted` defers validation to the caller's own infrastructure. The service turns that documented distinction into a synchronous admission check: because the `cloudfront` mode requires CloudFront to serve the HTTP validation token at the domain itself, the domain must already resolve to a CloudFront resource at create time. Recorded behavior: the create is refused synchronously with 400 InvalidArgument, message "The provided Domain Name is not valid. Could not verify Domain Name ownership. It may not be pointing to a valid CloudFront resource." — that sentence is entirely fixed text for this error class, with NO value interpolated from the request (no domain, tenant name, distribution id or connection-group id appears in it). No tenant and no certificate are created: GetDistributionTenantByDomain for that domain afterwards is 404 EntityNotFound, message "The specified distribution tenant doesn't exist." (also wholly fixed text). The same request with ValidationTokenHost="self-hosted" is accepted, which is the control that proves the refusal is keyed on the token host and not on the domain string.
Emulator: the call SUCCEEDS with HTTP 201 and returns a fully-formed DistributionTenant for the unverifiable domain — Status "InProgress", Domains [{Domain: <the unverifiable domain>, Status: "active"}], plus a minted tenant id, ARN, ConnectionGroupId and ETag. No validation of ValidationTokenHost against domain reachability happens at all, so the `cloudfront` and `self-hosted` modes are indistinguishable.
Why: a client that provisions multi-tenant domains with CloudFront-hosted token validation is told every tenant was created, including ones whose DNS does not point at CloudFront. On AWS those creates are hard 400s, so (a) the client's InvalidArgument handling for the "point your DNS at CloudFront first" workflow never runs and is never exercised offline, and (b) the emulator reports a tenant and an "active" domain for a certificate request that could never be validated — the deployment bug AWS surfaces immediately is hidden until production. It also makes the two ValidationTokenHost modes behaviourally identical, so no test can assert which mode an orchestrator selected.
Fix: the domain/ManagedCertificateRequest admission check in the CreateDistributionTenant handler (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_distribution_tenant.rs), with the per-code fixed message constant served verbatim; the state slice it validates against is the tenant/domain store in that crate's src/state.rs. Nothing must be persisted on the refusal path, so GetDistributionTenantByDomain stays EntityNotFound.
Traces: cloudfront.managed-certificate-status-lifecycle-transitions.

## Evidence
Check cloudfront.managed-certificate-status-lifecycle-transitions, verbatim replay tail:

AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, 'HTTPHeaders': {'content-type': 'application/xml', 'etag': 'EFGU4I109B1T52', ...}}, 'ETag': 'EFGU4I109B1T52', 'DistributionTenant': {'Id': 'dt_RKD77TQ89JF12', 'DistributionId': 'E7IYCGPKTANSF3', 'Name': 'sim2real-tenant-rejected-...', 'Arn': 'arn:aws:cloudfront::000000000000:distribution-tenant/dt_RKD77TQ89JF12', 'Domains': [{'Domain': 'sim2real-mc-rejected-....sim2real-cftest.net', 'Status': 'active'}], 'Tags': {'Items': []}, 'ConnectionGroupId': 'cg_GZNI4PND7H2S3', 'Enabled': True, 'Status': 'InProgress'}}

The failing call is cf.create_distribution_tenant(DistributionId=<multi-tenant distribution>, Name=<new>, Domains=[{'Domain': <domain>}], ConnectionGroupId=<connection group>, ManagedCertificateRequest={'ValidationTokenHost': 'cloudfront', 'PrimaryDomainName': <same domain>}).

Expected (recorded AWS behavior): HTTP 400, error code InvalidArgument, message "The provided Domain Name is not valid. Could not verify Domain Name ownership. It may not be pointing to a valid CloudFront resource." — fixed text, no request values interpolated. Then GetDistributionTenantByDomain(Domain=<that domain>) must answer 404 EntityNotFound with message "The specified distribution tenant doesn't exist." (the check asserts this effect-absence immediately after, and never reached it).

Positive control that passed in the same run: the identical shape of request with ManagedCertificateRequest={'ValidationTokenHost': 'self-hosted', ...} was accepted, and GetManagedCertificateDetails(Identifier=<tenant>) reported CertificateStatus 'pending-validation' — so the divergence is specifically the missing refusal for the 'cloudfront' token host, not a general certificate gap.

Identifiers above (dt_RKD77TQ89JF12, E7IYCGPKTANSF3, cg_GZNI4PND7H2S3, the generated domain and the ETag) are per-run emulator values quoted only to show the response shape; they are not part of the requirement.

## Steps to Reproduce
1. cloudfront CreateDistribution with ConnectionMode='tenant-only' and a TenantConfig, to get a multi-tenant distribution id.
2. cloudfront CreateConnectionGroup to get a connection-group id.
3. cloudfront CreateDistributionTenant(DistributionId=<step 1>, Name=<new name>, Domains=[{'Domain': '<any domain that does not resolve to a CloudFront resource>'}], ConnectionGroupId=<step 2>, ManagedCertificateRequest={'ValidationTokenHost': 'cloudfront', 'PrimaryDomainName': '<the same domain>'}).
   Observed: HTTP 201 with a DistributionTenant whose single domain reads Status 'active'.
   Expected: HTTP 400 InvalidArgument, message "The provided Domain Name is not valid. Could not verify Domain Name ownership. It may not be pointing to a valid CloudFront resource."
4. cloudfront GetDistributionTenantByDomain(Domain='<the same domain>') must then answer 404 EntityNotFound, message "The specified distribution tenant doesn't exist." (nothing persisted by the refused create).
5. Control (must keep succeeding): the same call with ManagedCertificateRequest={'ValidationTokenHost': 'self-hosted', ...} is accepted and GetManagedCertificateDetails reports CertificateStatus 'pending-validation'.

Or replay the check id in trace_ids.
