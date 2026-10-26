Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-domain-conflicts-detects-collision
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.list-domain-conflicts-detects-collision|_error_code_and_message#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp!r}")
Issue-Commit: 3fc7e3e957d43d95045575b42258a9f9788f891e

# CreateDistributionTenant binds a tenant to a domain that was never proven under DNS control (201 instead of 400 InvalidArgument "The request contains an invalid domain name.")

> _Note: this issue was identified against the code base at commit `3fc7e3e957d43d95045575b42258a9f9788f891e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistributionTenant

## Description
One net-new root cause in the cloudfront service. Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUE (does NOT cover this cause): 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, cause 6 of 10, records that tenant DOMAIN STATUS is not modelled — a domain backed by a still-pending ManagedCertificateRequest reads Status 'active' immediately instead of 'inactive'. That is the post-create representation of an accepted tenant. The cause below is the ADMISSION rule on a different input shape: a CreateDistributionTenant that supplies Customizations.Certificate.Arn (an imported ACM certificate, no ManagedCertificateRequest at all) for a domain this account has never bound a CloudFront resource to must be REFUSED outright, and here it is accepted with HTTP 201. The failing check is marked neither ALREADY FILED nor OWNED in the replay report.

=== Cause 1 of 1: CreateDistributionTenant accepts an arbitrary, unvalidated domain ===
AWS: Smithy CreateDistributionTenant (POST /2020-05-31/distribution-tenant, success code 201) takes DistributionId, Name and Domains as @required and models InvalidArgument (client, httpError 400), CNAMEAlreadyExists (409), EntityAlreadyExists (409), EntityLimitExceeded (400), EntityNotFound (404), InvalidAssociation (409), InvalidTagging (400) and AccessDenied (403). The model does not state the domain rule, so for the rule itself this is recorded behavior only: AWS requires the domain named in Domains to have been previously proven under the caller's DNS control before it will bind a tenant to it, and importing a self-signed certificate into ACM (which skips ACM's own domain validation) does not satisfy that requirement. The recorded refusal is HTTP 400, error code InvalidArgument, message "The request contains an invalid domain name." — that whole sentence is FIXED text for this error class and carries no value from the request or from state. Nothing is created: GetDistributionTenantByDomain(Domain=<that domain>) is then HTTP 404 EntityNotFound with the fixed message "The specified distribution tenant doesn't exist."
Emulator: the same call answers HTTP 201 and returns a fully-populated DistributionTenant for the fabricated domain — Id 'dt_<generated>', Arn 'arn:aws:cloudfront::<account>:distribution-tenant/<id>', Domains [{'Domain': '<the fabricated domain>', 'Status': 'active'}], ConnectionGroupId 'cg_<generated>', Status 'InProgress'. No domain-ownership admission check runs at all, and the domain is additionally reported 'active' rather than any pending state.
Why: the whole domain-validation gate of multi-tenant CloudFront is invisible. A client that wires a tenant to a domain it does not control is told the tenant exists and is active; against AWS that deployment is a hard 400, so a configuration error (typo'd domain, domain owned by someone else, validation step never performed) is certified by the emulator and only fails in production. The client's InvalidArgument handling for this operation never runs, and the downstream domain-conflict surface is unreachable because a fabricated domain can be bound freely and so can never be observed to collide.
Fix: the admission validation of the CreateDistributionTenant handler — resolve every entry of Domains against the account's proven/validated-domain state (the same state GetDistributionTenantByDomain and the domain-conflict surface read) BEFORE persisting anything, and refuse with 400 InvalidArgument and the fixed message above when a domain is not proven. Supplying Customizations.Certificate.Arn must not be treated as proof of domain control.
Traces: cloudfront.list-domain-conflicts-detects-collision.

## Evidence
Replay output, verbatim tail of cloudfront.list-domain-conflicts-detects-collision:

  File "check_cloudfront_list_domain_conflicts_detects_collision.py", line 201, in run
    code, message = _error_code_and_message(
        lambda: _create_distribution_tenant(
            ctx, mt_dist_id, domain, Customizations={"Certificate": {"Arn": tenant_cert_arn}}
        )
    )
  File "check_cloudfront_list_domain_conflicts_detects_collision.py", line 175, in _error_code_and_message
    raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp!r}")
  AssertionError: expected the call to be rejected, but it succeeded: {'Id': 'dt_...', 'DistributionId': 'E...', 'Name': 'sim2real-cf-tenant-...', 'Arn': 'arn:aws:cloudfront::000000000000:distribution-tenant/dt_...', 'Domains': [{'Domain': 'sim2real-cf-domconflict....example.com', 'Status': 'active'}], 'Tags': {...}, 'Customizations': {'Certificate': {'Arn': 'arn:aws:acm:us-west-2:000000000000:certificate/...'}}, 'ConnectionGroupId': 'cg_...', 'CreatedTime': ..., 'LastModifiedTime': ..., 'Enabled': False, 'Status': 'InProgress'}

(The generated ids, the minted domain label, the certificate ARN and the timestamps above are per-run values, not part of the requirement.)

Expected, per the recorded AWS behavior the check encodes:
  HTTP 400, Error.Code == "InvalidArgument", Error.Message == "The request contains an invalid domain name."
and then, through a different operation:
  GetDistributionTenantByDomain(Domain=<that domain>) -> HTTP 404, Error.Code == "EntityNotFound", Error.Message == "The specified distribution tenant doesn't exist."

Positive control inside the same run succeeded: the multi-tenant parent distribution (ConnectionMode=tenant-only, Enabled=True) was created and deployed normally, so the expected refusal is a domain-validation refusal and not a permissions or setup failure.

## Steps to Reproduce
1. cloudfront CreateDistributionWithTags for a multi-tenant parent: DistributionConfig with ConnectionMode='tenant-only', Enabled=True, one custom origin, DefaultCacheBehavior with a managed CachePolicyId. Note the returned distribution Id.
2. Mint a domain the account has never bound any CloudFront resource to, e.g. '<random-label>.example.com'.
3. acm ImportCertificate with a locally generated self-signed cert+key whose CN/SAN is that domain. Note the CertificateArn.
4. cloudfront CreateDistributionTenant(DistributionId=<step 1 id>, Name=<new name>, Domains=[{'Domain': <step 2 domain>}], Enabled=False, Customizations={'Certificate': {'Arn': <step 3 arn>}}).
   Observed: HTTP 201 with a DistributionTenant whose Domains[0].Status is 'active'.
   Expected: HTTP 400, InvalidArgument, message "The request contains an invalid domain name."
5. cloudfront GetDistributionTenantByDomain(Domain=<step 2 domain>).
   Expected: HTTP 404 EntityNotFound, message "The specified distribution tenant doesn't exist." (unreachable today, because step 4 created the tenant).
Or replay the check id in trace_ids.
