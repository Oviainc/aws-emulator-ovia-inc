Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.connection-mode-change-on-update-rejected cloudfront.distribution-tenant-customization-unicode-domain-preserved cloudfront.distribution-tenant-on-disabled-distribution-rejected cloudfront.e2e-distribution-tenant-webacl-invalidation-flow cloudfront.op-contract-associate-distribution-tenant-web-acl cloudfront.verify-dns-configuration-invalid-domain-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|UpdateDistribution|InvalidArgument
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront: REOPENS — UpdateDistribution refuses a multi-tenant distribution's own round-tripped config with InvalidArgument "You can't specify the SmoothStreaming field for a multi-tenant distribution.", blocking six tenant checks

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
REOPENS 20261102T102137Z-cloudfront-nine-control-plane-validation-gaps-createdistribution-updatedistribution.md: its fix left the call refused. That issue carried cloudfront.connection-mode-change-on-update-rejected and cloudfront.distribution-tenant-on-disabled-distribution-rejected in its trace ids (Causes 1 and 5) and asked UpdateDistribution to start validating; the validation that was added now refuses the legitimate call, and four further tenant checks now fail on the same refusal.

Cause 1 of 1: UpdateDistribution rejects a tenant-only DistributionConfig that the emulator's own GetDistributionConfig just served.
AWS: Smithy (cloudfront 2020-05-31): UpdateDistribution is PUT /2020-05-31/distribution/{Id}/config and models InvalidArgument (httpError 400). CacheBehavior.SmoothStreaming and DefaultCacheBehavior.SmoothStreaming are documented "This field only supports standard distributions. You can't specify this field for multi-tenant distributions" — so for a ConnectionMode=tenant-only distribution AWS does not accept a SmoothStreaming value, and correspondingly it does NOT emit one in that distribution's config. The round trip AWS guarantees is: GetDistributionConfig -> mutate one member -> UpdateDistribution under the returned ETag succeeds. Where a tenant-only update IS illegal for another reason, the refusal is the one that reason names — e.g. flipping ConnectionMode is 400 IllegalUpdate "You can't update the ConnectionMode field for this distribution." (fixed text, no variable part).
Emulator: GetDistributionConfig on a tenant-only distribution serves a config that CONTAINS SmoothStreaming (the create path defaults it in), and UpdateDistribution then refuses that same body with 400 InvalidArgument and the message "You can't specify the SmoothStreaming field for a multi-tenant distribution." The two halves of the contract contradict each other, so no multi-tenant distribution can ever be updated. On cloudfront.connection-mode-change-on-update-rejected this refusal also preempts the IllegalUpdate the check expects, so the ConnectionMode-immutability contract is still unverifiable.
Why: a client cannot modify a multi-tenant distribution at all — not its comment, not its tenant config, not its customizations — because the only config the service will give it is one the service will not take back. Every tenant provisioning, web-ACL association and DNS-verification flow that touches UpdateDistribution dies on the setup step, and a caller that reasonably trusts the read-modify-write cycle sees a 400 it cannot act on.
Fix: the tenant-only branch of the distribution config store/serializer must not materialise SmoothStreaming (and the other standard-distribution-only flags) for a ConnectionMode=tenant-only distribution, and UpdateDistribution's multi-tenant validation must only reject a SmoothStreaming value the CALLER supplied, not one the service defaulted in — the cloudfront UpdateDistribution handler's validation plus the distribution config state/serializer.
Traces: cloudfront.connection-mode-change-on-update-rejected, cloudfront.distribution-tenant-customization-unicode-domain-preserved, cloudfront.distribution-tenant-on-disabled-distribution-rejected, cloudfront.e2e-distribution-tenant-webacl-invalidation-flow, cloudfront.op-contract-associate-distribution-tenant-web-acl, cloudfront.verify-dns-configuration-invalid-domain-rejected.

## Evidence
All six checks abort on the same refusal, raised from the UpdateDistribution step that follows a GetDistributionConfig on a tenant-only distribution:

botocore.errorfactory.InvalidArgument: An error occurred (InvalidArgument) when calling the UpdateDistribution operation: You can't specify the SmoothStreaming field for a multi-tenant distribution.

Expected on cloudfront.connection-mode-change-on-update-rejected: 400 IllegalUpdate with message "You can't update the ConnectionMode field for this distribution." (the check asserts (code, message) == ("IllegalUpdate", that sentence)), the stored ConnectionMode still 'tenant-only' and the ETag unadvanced.
Expected on the other five: a 2xx UpdateDistribution, because the submitted body is the distribution's own config as GetDistributionConfig served it, with one member changed.
Actual: HTTP 400, code InvalidArgument, message "You can't specify the SmoothStreaming field for a multi-tenant distribution." The whole sentence is fixed text except the member name SmoothStreaming, which names the offending member; the defect is that the member was never supplied by the caller — the emulator's own readback put it there.

## Steps to Reproduce
1. Point a cloudfront client at the emulator (us-east-1).
2. CreateDistribution with ConnectionMode="tenant-only" (an OAC'd origin, TenantConfig with the parameters the origin path references).
3. resp = GetDistributionConfig(Id=<dist id>) and observe that resp["DistributionConfig"]["DefaultCacheBehavior"] contains SmoothStreaming.
4. Change nothing but Comment, then UpdateDistribution(Id, IfMatch=resp["ETag"], DistributionConfig=resp["DistributionConfig"]).
5. Observe 400 InvalidArgument "You can't specify the SmoothStreaming field for a multi-tenant distribution." instead of a 2xx.
6. Repeat with ConnectionMode flipped to "direct" and observe the same refusal instead of the expected 400 IllegalUpdate "You can't update the ConnectionMode field for this distribution."
7. Or replay the six check ids in trace_ids.
