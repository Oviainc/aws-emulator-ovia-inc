Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfoac-tenant-discovery-route-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfoac-tenant-discovery-route-mistakes cloudfront-cfoac-tenant-only-distribution-reference-diagnosis cloudfront-cfode-attach-rewrite-request-shape-mistakes cloudfront-cfode-rewrite-fix-function-state-and-etag-chain cloudfront-cfode-distribution-lookup-identifier-mistakes cloudfront-icc-readback-identifier-and-paging-mistakes
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|ListDistributionTenants|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|ListDistributionTenants|4xx->2xx|status
Issue-Key: cloudfront|CreateDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|CreateDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateDistributionTenant|4xx->4xx|status
Issue-Key: cloudfront|CreateDistributionTenant|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateDistributionTenant|4xx->2xx|status
Issue-Key: cloudfront|ListDistributionTenants|2xx->2xx|$.ListDistributionTenantsResult.DistributionTenantList
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|status
Issue-Key: cloudfront|ListDistributions|2xx->4xx|$.DistributionList
Issue-Key: cloudfront|ListDistributions|2xx->4xx|status
Issue-Key: cloudfront|ListInvalidations|2xx->4xx|$.InvalidationList
Issue-Key: cloudfront|ListInvalidations|2xx->4xx|status
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFront request validation accepts invalid requests and rejects valid ones

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDistributionTenants

## Description
Seven net-new wire divergences where the emulator's admission decision is simply inverted relative to AWS: requests AWS refuses are accepted with a 2xx (and their side effects applied), and requests AWS accepts are refused with a 400. Both directions come from the same missing layer — per-operation precondition checks on tenant/distribution requests and on pagination inputs. The open issue 20261102T102137Z-cloudfront-nine-control-plane-validation-gaps-createdistribution-updatedistribution.md covers a different set of replay keys and does not cover these.

Cause 1 of 7: ListDistributionTenants with no association filter succeeds instead of being refused.
AWS: Smithy: ListDistributionTenants input has optional AssociationFilter (DistributionTenantAssociationFilter) and binds InvalidArgument (httpError 400). Recorded refusal: 400 with Type `Sender`, Code `InvalidArgument`, Message "Exactly one association filter must be set." — entirely fixed text, no variable parts. AWS requires exactly one filter field inside AssociationFilter.
Emulator: answers 200 with a tenant list and no ErrorResponse.
Why: a client that relied on the refusal to catch its own malformed filter instead receives an unfiltered list and acts on tenants it never asked about.
Fix: ListDistributionTenants' input validation — reject an AssociationFilter that sets zero or more than one field.
Traces: cloudfront-cfoac-tenant-discovery-route-mistakes#task#1.

Cause 2 of 7: CreateDistributionTenant against a disabled multi-tenant distribution reports the wrong precondition.
AWS: Smithy: CreateDistributionTenant binds InvalidAssociation (httpError 409) and InvalidArgument (400). Recorded refusal: 409, Code `InvalidAssociation`, Message "The specified multi-tenant distribution is disabled." (fixed text). AWS evaluates the referenced distribution's enabled state before it validates tenant parameters.
Emulator: answers 400, Code `InvalidArgument`, Message "An argument is invalid: the parameter TenantPath is required by the distribution and the request supplies no value for it".
Why: the client is told to supply a parameter, supplies it, and still fails — the real blocker (the distribution is disabled) is never reported, and the 409-vs-400 class difference misroutes conflict handling.
Fix: ordering and the missing check in CreateDistributionTenant's validation: evaluate the target distribution's enabled state first and raise InvalidAssociation.
Traces: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#9.

Cause 3 of 7: CreateDistributionTenant against a disabled multi-tenant distribution succeeds with 201.
AWS: as Cause 2 — recorded refusal 409 / `InvalidAssociation` / "The specified multi-tenant distribution is disabled."
Emulator: answers 201 and creates the tenant, with no ErrorResponse.
Why: a tenant now exists that AWS would never have created, under a distribution that cannot serve it. The caller's subsequent teardown and quota accounting are both wrong.
Fix: CreateDistributionTenant's validation must check the referenced distribution's enabled state on every path, not only when tenant parameters are absent.
Traces: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#10.

Cause 4 of 7: ListDistributionTenants returns a tenant that should never have been created.
AWS: Smithy: ListDistributionTenantsResult.DistributionTenantList is a list. Recorded behavior: after the create in Cause 3 is refused, the list is empty.
Emulator: returns a populated DistributionTenantList containing a DistributionTenantSummary (expected an empty list, got a tenant entry).
Why: this is the observable consequence of the accepted-but-invalid create — the emulator's tenant inventory diverges from AWS's for the rest of the session, so any client enumerating tenants sees state AWS does not have.
Fix: same as Cause 3 (CreateDistributionTenant validation); verify by re-reading the list after the create is correctly refused.
Traces: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#11.

Cause 5 of 7: UpdateDistribution accepts a config with no PriceClass instead of refusing it.
AWS: Smithy: UpdateDistribution binds IllegalUpdate (httpError 400). Recorded refusal: 400 with Type `Sender`, Code `IllegalUpdate`, Message "Price class is missing for the resource" — fixed text. A DistributionConfig submitted to UpdateDistribution must carry the full config, including PriceClass.
Emulator: answers 200 and applies the partial config.
Why: a caller that submitted a partial config believes a full update landed; the distribution is left with whatever PriceClass the emulator invented, so the resource diverges from both the submitted config and AWS. 2 affected steps.
Fix: UpdateDistribution's validation — require the complete DistributionConfig and raise IllegalUpdate for a missing required sub-member.
Traces: cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#7 (2 affected steps).

Cause 6 of 7: ListDistributions with MaxItems=0 is refused with 400 instead of returning an empty page.
AWS: Smithy: ListDistributions input MaxItems is an httpQuery integer with no @range constraint, and the operation binds only InvalidArgument (400); output DistributionList is the httpPayload. Recorded behavior: 200 with DistributionList {Marker: '', MaxItems: '0', IsTruncated: 'true', Quantity: '0'} — a zero page size is a legal request that yields an empty, truncated page.
Emulator: answers 400 and no DistributionList.
Why: a paginating client that computes a page size of zero (a common edge case when a caller-supplied limit is already satisfied) gets a hard error instead of an empty page, so the pagination loop aborts rather than terminating.
Fix: ListDistributions' input validation must not reject MaxItems=0, and the handler must return an empty truncated DistributionList.
Traces: cloudfront-cfode-distribution-lookup-identifier-mistakes#task#9.

Cause 7 of 7: ListInvalidations with MaxItems=0 is refused with 400 instead of returning an empty page.
AWS: Smithy: ListInvalidations input MaxItems is an httpQuery integer, errors AccessDenied/InvalidArgument/NoSuchDistribution; output InvalidationList is the httpPayload. Recorded behavior: 200 with InvalidationList {Marker: '', MaxItems: '0', IsTruncated: 'true', Quantity: '0'}.
Emulator: answers 400 and no InvalidationList.
Why: as Cause 6, on the invalidation list — the same shared pagination validation is wrong, so no CloudFront list operation can be driven with a zero page size.
Fix: the shared CloudFront pagination input validation used by the list handlers.
Traces: cloudfront-icc-readback-identifier-and-paging-mistakes#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 78.2%, 102 divergence clusters, all cloudfront).

Cause 1 — cloudfront-cfoac-tenant-discovery-route-mistakes#task#1 ListDistributionTenants: expected 400 with ErrorResponse {Error:{Type:'Sender', Code:'InvalidArgument', Message:'Exactly one association filter must be set.'}}; got 200 and no ErrorResponse.
Cause 2 — cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#9 CreateDistributionTenant: expected 409 / 'InvalidAssociation' / 'The specified multi-tenant distribution is disabled.'; got 400 / 'InvalidArgument' / 'An argument is invalid: the parameter TenantPath is required by the distribution and the request supplies no value for it'.
Cause 3 — #task#10 CreateDistributionTenant: expected 409 / 'InvalidAssociation' / 'The specified multi-tenant distribution is disabled.'; got 201 and no ErrorResponse.
Cause 4 — #task#11 ListDistributionTenants: $.ListDistributionTenantsResult.DistributionTenantList type_mismatch — expected '' (empty), got a DistributionTenantSummary for the tenant created in task#10.
Cause 5 — cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#7 UpdateDistribution: expected 400 with {Error:{Type:'Sender', Code:'IllegalUpdate', Message:'Price class is missing for the resource'}}; got 200 and no ErrorResponse (also cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#5).
Cause 6 — cloudfront-cfode-distribution-lookup-identifier-mistakes#task#9 ListDistributions: expected 200 with DistributionList {Marker:'', MaxItems:'0', IsTruncated:'true', Quantity:'0'}; got 400 and DistributionList missing.
Cause 7 — cloudfront-icc-readback-identifier-and-paging-mistakes#task#5 ListInvalidations: expected 200 with InvalidationList {Marker:'', MaxItems:'0', IsTruncated:'true', Quantity:'0'}; got 400 and InvalidationList missing.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-cfoac-tenant-discovery-route-mistakes and inspect step task#1 (representative).
4. Replay cloudfront-cfoac-tenant-only-distribution-reference-diagnosis steps task#9, task#10 and task#11 in order (task#11 only shows the divergence once task#10 has run).
5. Replay cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#7, cloudfront-cfode-distribution-lookup-identifier-mistakes#task#9 and cloudfront-icc-readback-identifier-and-paging-mistakes#task#5.
6. Compare each step's status and body against the recorded response: the four over-permissive steps must refuse, and the two MaxItems=0 list steps must answer 200 with an empty truncated list. All seven causes must match before closing.
