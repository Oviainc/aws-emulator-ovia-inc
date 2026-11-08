Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.connection-mode-change-on-update-rejected cloudfront.create-invalidation-rejects-malformed-paths cloudfront.direct-distribution-with-tenant-parameters-rejected cloudfront.tenant-only-origin-path-with-undefined-parameter-rejected cloudfront.distribution-tenant-on-disabled-distribution-rejected cloudfront.oac-name-reuse-rejected-and-first-survives cloudfront.update-distribution-requires-the-complete-config-rejects-a-sparse-body cloudfront.distribution-caller-reference-rejects-reuse-and-change cloudfront.invalidation-paths-differing-only-in-case-stay-distinct
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.connection-mode-change-on-update-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.create-invalidation-rejects-malformed-paths|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.direct-distribution-with-tenant-parameters-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.tenant-only-origin-path-with-undefined-parameter-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.distribution-tenant-on-disabled-distribution-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.oac-name-reuse-rejected-and-first-survives|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.update-distribution-requires-the-complete-config-rejects-a-sparse-body|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront.distribution-caller-reference-rejects-reuse-and-change|_error_response#19|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: cloudfront.invalidation-paths-differing-only-in-case-stay-distinct|run#7|AssertionError|assert echoed["Quantity"] == 3 and set(echoed["Items"]) == want, (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudfront: nine control-plane validation gaps — CreateDistribution/UpdateDistribution/CreateInvalidation/CreateOriginAccessControl/CreateDistributionTenant accept requests AWS refuses

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
Nine net-new root causes, all in the cloudfront control plane: an operation ACCEPTS (2xx) a request real AWS refuses, or stores something AWS does not store. Related but NOT covering issues already open: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md, 20261029T212223Z-cloudfront-accepts-requests-aws-refuses-limits-argument-and.md and 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md — none of them carries the replay keys below.

Cause 1 of 9: UpdateDistribution accepts a ConnectionMode change.
AWS: ConnectionMode (DistributionConfig.ConnectionMode, enum direct|tenant-only) is immutable after create; UpdateDistribution refuses the change with 400 IllegalUpdate (Smithy: UpdateDistribution error IllegalUpdate, httpError 400) and the message "You can't update the ConnectionMode field for this distribution." (fixed text, no variable part). The stored ConnectionMode and the ETag stay as they were.
Emulator: the update SUCCEEDS (2xx, no error raised) on an otherwise-complete direct-mode body built from a tenant-only distribution's own config.
Why: a client can silently convert a multi-tenant distribution into a standard one; every tenant bound to it becomes unroutable and no error ever tells the caller.
Fix: UpdateDistribution's validation (immutable-field check against the stored config) in the cloudfront ops handler for UpdateDistribution.
Traces: cloudfront.connection-mode-change-on-update-rejected.

Cause 2 of 9: CreateInvalidation accepts a degenerate Paths batch.
AWS: an InvalidationBatch whose Paths declares zero paths (Quantity 0, no Items) or whose sole path is the empty string is refused 400 InvalidArgument (Smithy: CreateInvalidation error InvalidArgument, httpError 400) with the message "Your request contains one or more invalid invalidation paths." (fixed text). Nothing is created, so ListInvalidations still shows only the earlier good invalidation.
Emulator: both calls SUCCEED and mint invalidations.
Why: a deploy script that builds an empty path list gets a success and believes it invalidated content that is still cached; the account also accumulates invalidations AWS would never have created.
Fix: CreateInvalidation's request validation (Paths.Quantity/Items) in the cloudfront CreateInvalidation handler.
Traces: cloudfront.create-invalidation-rejects-malformed-paths.

Cause 3 of 9: CreateDistribution accepts a templated OriginPath on a ConnectionMode=direct distribution.
AWS: tenant parameter templating ({{<parameter>}} in Origins.Items[].OriginPath) is only valid for a multi-tenant (tenant-only) distribution. A direct distribution carrying it is refused 400 InvalidArgument (Smithy: CreateDistribution error InvalidArgument, httpError 400) with "The parameter originPath contains one or more parameters that are not valid." (fixed text; the member name originPath is the offending member). No distribution is left behind.
Emulator: the create SUCCEEDS and a direct distribution with a templated origin path is persisted.
Why: the invalid template is never substituted at the edge, so every request to that distribution hits a literal "/{{...}}" origin path — a failure the caller would have learned about at create time on AWS.
Fix: CreateDistribution's tenant-parameter validation (ConnectionMode vs templated members) in the cloudfront CreateDistribution handler.
Traces: cloudfront.direct-distribution-with-tenant-parameters-rejected.

Cause 4 of 9: CreateDistribution accepts a tenant-only OriginPath naming a parameter TenantConfig does not declare.
AWS: every parameter referenced in a multi-tenant DistributionConfig must appear in TenantConfig.ParameterDefinitions; otherwise the create is refused 400 InvalidArgument with "A parameter in the DistributionConfig doesn't exist in the ParameterDefinitions list. Remove the parameter and try again." (fixed text). No distribution is left behind.
Emulator: the create SUCCEEDS with TenantConfig entirely absent while OriginPath still references the parameter.
Why: tenants created under that distribution can never resolve their path, and the operator gets the failure at request time instead of at configuration time.
Fix: CreateDistribution's tenant-parameter validation (referenced parameters vs TenantConfig.ParameterDefinitions) in the cloudfront CreateDistribution handler.
Traces: cloudfront.tenant-only-origin-path-with-undefined-parameter-rejected.

Cause 5 of 9: CreateDistributionTenant accepts a tenant on a DISABLED multi-tenant distribution.
AWS: the tenant create depends on the PARENT's state — a disabled multi-tenant distribution refuses it with 409 InvalidAssociation (Smithy: CreateDistributionTenant error InvalidAssociation, httpError 409) and the message "The specified multi-tenant distribution is disabled." (fixed text). The tenant is not listed afterwards; enabling the parent makes the identical request succeed.
Emulator: the call SUCCEEDS against the disabled parent.
Why: a client cannot distinguish a tenant that is actually attachable from one that is not, so a provisioning flow reports success for tenants AWS would have refused.
Fix: CreateDistributionTenant's parent-state validation in the cloudfront CreateDistributionTenant handler.
Traces: cloudfront.distribution-tenant-on-disabled-distribution-rejected.

Cause 6 of 9: CreateOriginAccessControl accepts a duplicate Name.
AWS: OAC name uniqueness is keyed on Name ALONE (not on (Name, OriginAccessControlOriginType)); a second create under a taken name is refused 409 OriginAccessControlAlreadyExists (Smithy: CreateOriginAccessControl error OriginAccessControlAlreadyExists, httpError 409) with "An origin access control with the same name already exists." (fixed text). The holder keeps its own OriginAccessControlOriginType, and freeing the name lets the other type succeed.
Emulator: the second create (same Name, OriginAccessControlOriginType "mediapackagev2" against a held "s3" name) SUCCEEDS.
Why: name-based lookups and idempotent provisioners now see two OACs under one name, and the holder's identity is no longer unique.
Fix: CreateOriginAccessControl's uniqueness check (index on Name) in the cloudfront CreateOriginAccessControl handler/state.
Traces: cloudfront.oac-name-reuse-rejected-and-first-survives.

Cause 7 of 9: UpdateDistribution accepts a sparse DistributionConfig instead of demanding the complete config.
AWS: UpdateDistribution is a full replace, not a merge. A body that omits a top-level setting the distribution carries is refused 400 IllegalUpdate naming the missing setting — for an omitted PriceClass the message is "Price class is missing for the resource" (fixed text; the leading noun names the omitted member). The ETag is not rotated and the stored config is untouched.
Emulator: the sparse update SUCCEEDS.
Why: a client that PUTs a partial body silently loses PriceClass, HttpVersion, IsIPV6Enabled and DefaultRootObject on the emulator while AWS would have refused the call, so the same code produces two different distributions.
Fix: UpdateDistribution's completeness validation in the cloudfront UpdateDistribution handler.
Traces: cloudfront.update-distribution-requires-the-complete-config-rejects-a-sparse-body.

Cause 8 of 9: a CreateDistribution replay under an already-used CallerReference is accepted after the distribution has been updated.
AWS: CallerReference's idempotent-replay window is an as-created privilege. Before any update, replaying the unmodified create returns the SAME distribution id. Once UpdateDistribution has run, a create under that reference — even resending the distribution's own current config — is refused 409 DistributionAlreadyExists (Smithy: CreateDistribution error DistributionAlreadyExists, httpError 409; DistributionConfig.CallerReference doc: "If CallerReference is a value that you already sent in a previous request to create a distribution, CloudFront returns a DistributionAlreadyExists error") with the message "The caller reference that you are using to create a distribution is associated with another distribution. Already exists: <existing distribution id>" (fixed text plus the id of the distribution already holding the reference).
Emulator: the post-update replay SUCCEEDS (no error raised).
Why: a retried deploy mints a second distribution (or silently re-creates) where AWS refuses, so retries are not idempotent and the caller's conflict-handling path is never exercised.
Fix: CreateDistribution's caller-reference bookkeeping (reference is consumed once the distribution has been updated) in the cloudfront CreateDistribution handler/state.
Traces: cloudfront.distribution-caller-reference-rejects-reuse-and-change.

Cause 9 of 9: CreateInvalidation does not collapse exact-duplicate paths.
AWS: invalidation paths are case SENSITIVE and stored as a set — paths differing only in case stay distinct entries, while an exact repeat collapses. Sending ["/images/logo.png", "/images/Logo.png", "/IMAGES/logo.png", "/images/logo.png"] is echoed as Quantity 3 over the three distinct case variants, and GetInvalidation reports the same three.
Emulator: the batch is stored verbatim — Quantity 4 with "/images/logo.png" listed twice.
Why: a client that computes a path list with repeats sees a Quantity and item list AWS would never return, so quota accounting and any diff of the echoed batch against the requested set are wrong.
Fix: CreateInvalidation's path normalisation (de-duplicate exact repeats, preserve case) in the cloudfront CreateInvalidation handler/state.
Traces: cloudfront.invalidation-paths-differing-only-in-case-stay-distinct.

## Evidence
Cause 1: expected ('IllegalUpdate', "You can't update the ConnectionMode field for this distribution."); got a successful UpdateDistribution — "AssertionError: the call succeeded; real AWS rejects it, so there is no error code".
Cause 2: expected 400 InvalidArgument "Your request contains one or more invalid invalidation paths." for paths [""] and for Quantity 0; got success for both — "AssertionError: the call succeeded; real AWS rejects it, so there is no error code".
Cause 3: expected 400 InvalidArgument "The parameter originPath contains one or more parameters that are not valid."; got success — "AssertionError: the call succeeded; real AWS rejects it, so there is no error code".
Cause 4: expected 400 InvalidArgument "A parameter in the DistributionConfig doesn't exist in the ParameterDefinitions list. Remove the parameter and try again."; got success.
Cause 5: expected 409 InvalidAssociation "The specified multi-tenant distribution is disabled."; got success.
Cause 6: expected 409 OriginAccessControlAlreadyExists "An origin access control with the same name already exists."; got success.
Cause 7: expected 400 IllegalUpdate "Price class is missing for the resource"; got success.
Cause 8: expected 409 DistributionAlreadyExists "The caller reference that you are using to create a distribution is associated with another distribution. Already exists: <id>"; got success.
Cause 9: CreateInvalidation stored {'Quantity': 4, 'Items': ['/images/logo.png', '/images/Logo.png', '/IMAGES/logo.png', '/images/logo.png']}; expected Quantity 3 over the three case variants.

## Steps to Reproduce
1. Point a cloudfront client at the emulator (us-east-1).
2. Cause 1: create a tenant-only distribution, GetDistributionConfig, set ConnectionMode=direct (completing the direct-mode fields), UpdateDistribution with the current ETag -> expect 400 IllegalUpdate.
3. Cause 2: CreateInvalidation on any distribution with Paths={Quantity:0} and with Paths={Quantity:1, Items:[""]} -> expect 400 InvalidArgument each time.
4. Cause 3: CreateDistribution with ConnectionMode=direct and Origins.Items[0].OriginPath="/{{TenantPath}}" -> expect 400 InvalidArgument.
5. Cause 4: CreateDistribution with ConnectionMode=tenant-only, that same templated OriginPath and no TenantConfig -> expect 400 InvalidArgument.
6. Cause 5: create a DISABLED tenant-only distribution, then CreateDistributionTenant against it -> expect 409 InvalidAssociation.
7. Cause 6: CreateOriginAccessControl Name=X type s3, then CreateOriginAccessControl Name=X type mediapackagev2 -> expect 409 OriginAccessControlAlreadyExists.
8. Cause 7: create a distribution declaring PriceClass/HttpVersion/IsIPV6Enabled/DefaultRootObject, then UpdateDistribution with a body omitting them -> expect 400 IllegalUpdate.
9. Cause 8: CreateDistribution with CallerReference R, UpdateDistribution once, then CreateDistribution again with R and the current config -> expect 409 DistributionAlreadyExists.
10. Cause 9: CreateInvalidation with Items ["/images/logo.png","/images/Logo.png","/IMAGES/logo.png","/images/logo.png"] -> expect Quantity 3 and three distinct items from both CreateInvalidation and GetInvalidation.
