Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.disassociate-distribution-tenant-web-acl-no-association-invalid-argument-rejected cloudfront.managed-cache-policy-delete-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.disassociate-distribution-tenant-web-acl-no-association-invalid-argument-rejected|run#13|AssertionError|assert missing_precondition == "InvalidIfMatchVersion", (
Issue-Key: cloudfront|GetCachePolicy|NoSuchCachePolicy
Issue-Commit: c47a25df631b118aeb39d6f2215b2ba176696642

# cloudfront: DisassociateDistributionTenantWebACL answers NotImplemented, and the AWS-managed cache-policy catalog is absent so GetCachePolicy 404s on a managed policy id (2 causes)

> _Note: this issue was identified against the code base at commit `c47a25df631b118aeb39d6f2215b2ba176696642`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCachePolicy

## Description
Two net-new root causes in the CloudFront emulator, both grounded in behavioral replay. A fixer must close BOTH before submitting.

=== Cause 1 of 2: DisassociateDistributionTenantWebACL is not served — it answers NotImplemented instead of the operation's modeled behavior ===

AWS: Smithy: DisassociateDistributionTenantWebACL, PUT /2020-05-31/distribution-tenant/{Id}/disassociate-web-acl, success code 200; input Id (@httpLabel, @required) and IfMatch (@httpHeader "If-Match"); output Id + ETag (@httpHeader); modeled errors AccessDenied 403, EntityNotFound 404, InvalidArgument 400, InvalidIfMatchVersion 400, PreconditionFailed 412. The model does not state which error the missing-precondition path takes, so for that the recorded behavior is the authority: with IfMatch omitted AWS refuses the call with HTTP 400 and error code InvalidIfMatchVersion. The rejection is produced by the request (a required precondition header is absent), so the requirement is "400 / InvalidIfMatchVersion naming the missing If-Match precondition"; the recorded text "The If-Match version is missing or not valid for the resource." is an example of that rule. Recorded behavior also establishes the success semantics the operation must have: with a valid IfMatch and no web ACL currently associated the call is an idempotent no-op (HTTP 200 with a fresh ETag, Customizations.WebAcl absent on the tenant both before and after), and after AssociateDistributionTenantWebACL with a CLOUDFRONT-scope WAFv2 ACL a subsequent disassociate clears Customizations.WebAcl again.

Emulator: every call to the operation is refused with error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator"). The replay asserted InvalidIfMatchVersion for the omitted-IfMatch call and got 'NotImplemented'. Because the operation never runs, the precondition rejection, the idempotent no-op arm and the clear-on-associated arm all do not exist, and the tenant's Customizations.WebAcl is never mutated.

Why: a client that disassociates a tenant web ACL sees a non-modeled error code for every call. Code that distinguishes the three real outcomes — precondition missing (InvalidIfMatchVersion, retry with a fresh ETag), nothing to do (200, no-op), association removed (200, Customizations.WebAcl gone) — cannot take any of its branches, and tenant WAF teardown is unreachable against the emulator.

Fix: the DisassociateDistributionTenantWebACL handler (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/disassociate_distribution_tenant_web_acl.rs), its IfMatch precondition validation, and the distribution-tenant Customizations.WebAcl state it must clear.

Note on overlap: an open scaffold issue names this same operation and handler file (filed from the sibling check cloudfront.disassociate-tenant-web-acl-when-none-associated-invalid-argument). That issue does not carry THIS check's replay key, and it states only "implement the stub" rather than the three fidelity requirements above, so this cause is filed with them spelled out. If the scaffold issue is fixed first, re-replay the trace id below to confirm the precondition arm and the no-op/clear arms as well.

Traces: cloudfront.disassociate-distribution-tenant-web-acl-no-association-invalid-argument-rejected

=== Cause 2 of 2: the AWS-managed cache-policy catalog is never seeded, so GetCachePolicy 404s on a managed policy id and the managed-policy IllegalDelete rule is unreachable ===

AWS: Smithy: GetCachePolicy, GET /2020-05-31/cache-policy/{Id}, success 200 with the CachePolicy as @httpPayload plus an ETag header; errors AccessDenied 403, NoSuchCachePolicy 404. DeleteCachePolicy, DELETE /2020-05-31/cache-policy/{Id}, success 204; errors AccessDenied 403, CachePolicyInUse 409, IllegalDelete 400, InvalidIfMatchVersion 400, NoSuchCachePolicy 404, PreconditionFailed 412. The model does not say which ids are service-managed, so for that the recorded behavior is the authority: AWS pre-provisions a fixed catalogue of managed cache policies in every account, GetCachePolicy on a managed id (e.g. 658327ea-f89d-4fab-a63d-7e88639e58f6) answers 200 with CachePolicyConfig.Name "Managed-CachingOptimized", ListCachePolicies(Type="managed") lists that policy with Type "managed", and DeleteCachePolicy on a managed id is refused with HTTP 400 and error code IllegalDelete — specifically NOT AccessDenied and not a success — with the policy still readable afterwards. That refusal is produced by the request (the target is a managed, read-only resource), so "400 / IllegalDelete" is the requirement; the recorded text "The request failed because delete is not allowed for this policy." is an example.

Emulator: GetCachePolicy on the managed id 658327ea-f89d-4fab-a63d-7e88639e58f6 is refused with HTTP 404 and error code NoSuchCachePolicy ("The specified cache policy does not exist."). The managed catalogue does not exist in state at all, so the managed policy cannot be read, cannot appear under ListCachePolicies(Type="managed"), and the IllegalDelete classification for a delete attempt against it can never be produced. The customer-owned leg of the same check is served correctly (create, delete, then 404 NoSuchCachePolicy on read-back), so this is specifically the absence of the managed catalogue rather than a broken cache-policy surface.

Why: managed cache policy ids are the normal way distributions are configured — a client that reads back the policy it referenced, or that enumerates managed policies to pick one, gets a 404 for a resource AWS guarantees exists in every account. A client that probes whether a policy is deletable gets "does not exist" instead of "delete is not allowed for this policy", which is a different remediation: one says re-create it, the other says it is service-owned and must be left alone. Infrastructure tooling that treats 404 as "drifted, re-create" will try to create a policy it must not own.

Fix: the CloudFront cache-policy state must seed the AWS-managed catalogue (ids, names and configs, classified Type="managed"), and the GetCachePolicy, ListCachePolicies and DeleteCachePolicy handlers must resolve against it — with DeleteCachePolicy classifying a managed target as IllegalDelete (400) and leaving it in place. Handler directory: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/.

Traces: cloudfront.managed-cache-policy-delete-rejected

## Evidence
Cause 1 — check cloudfront.disassociate-distribution-tenant-web-acl-no-association-invalid-argument-rejected (verdict: mismatch):
  Expected (AWS): DisassociateDistributionTenantWebACL(Id=<tenant>) with IfMatch omitted -> HTTP 400, error code InvalidIfMatchVersion; then DisassociateDistributionTenantWebACL(Id=<tenant>, IfMatch=<etag>) with nothing associated -> HTTP 200, fresh ETag, GetDistributionTenant shows no Customizations.WebAcl; then associate a CLOUDFRONT-scope WAFv2 ACL and disassociate -> Customizations.WebAcl cleared.
  Actual (emulator): AssertionError: expected InvalidIfMatchVersion when IfMatch is omitted, got 'NotImplemented'. The emulator answers error code NotImplemented for every call to this operation, so none of the later arms were reached.

Cause 2 — check cloudfront.managed-cache-policy-delete-rejected (verdict: error):
  Expected (AWS): GetCachePolicy(Id="658327ea-f89d-4fab-a63d-7e88639e58f6") -> HTTP 200 with CachePolicy.CachePolicyConfig.Name == "Managed-CachingOptimized" and an ETag; ListCachePolicies(Type="managed") includes that id with Type == "managed"; DeleteCachePolicy(Id=same, IfMatch=<etag>) -> HTTP 400, error code IllegalDelete, and GetCachePolicy still returns the policy.
  Actual (emulator): botocore.errorfactory.NoSuchCachePolicy: An error occurred (NoSuchCachePolicy) when calling the GetCachePolicy operation: The specified cache policy does not exist. (HTTP 404) — raised on the unguarded read of the managed policy id, so the IllegalDelete assertion was never reached.
  Control that did pass in the same check: a cache policy the check creates is deletable, and reading it back after delete correctly yields 404 NoSuchCachePolicy — the customer-owned path works; only the managed catalogue is missing.

## Steps to Reproduce
Cause 1:
1. Create a tenant-only (ConnectionMode="tenant-only") distribution, a connection group, and a distribution tenant on them.
2. Call DisassociateDistributionTenantWebACL(Id=<tenant id>) with NO IfMatch header.
   Expect HTTP 400 / InvalidIfMatchVersion. Observed: error code NotImplemented.
3. Call DisassociateDistributionTenantWebACL(Id=<tenant id>, IfMatch=<tenant ETag>) with no web ACL associated.
   Expect HTTP 200 and a fresh ETag, with GetDistributionTenant reporting no Customizations.WebAcl.
4. Associate a CLOUDFRONT-scope WAFv2 web ACL via AssociateDistributionTenantWebACL, then disassociate it with the returned ETag.
   Expect Customizations.WebAcl to be absent again.

Cause 2:
1. CreateCachePolicy with any valid config; DeleteCachePolicy it with the returned ETag; GetCachePolicy it again and confirm HTTP 404 / NoSuchCachePolicy (this leg already passes).
2. Call GetCachePolicy(Id="658327ea-f89d-4fab-a63d-7e88639e58f6") — the AWS-managed "Managed-CachingOptimized" policy.
   Expect HTTP 200 with CachePolicyConfig.Name == "Managed-CachingOptimized". Observed: HTTP 404 / NoSuchCachePolicy.
3. Call ListCachePolicies(Type="managed") and expect that id present with Type == "managed".
4. Call DeleteCachePolicy(Id="658327ea-f89d-4fab-a63d-7e88639e58f6", IfMatch=<its ETag>).
   Expect HTTP 400 / IllegalDelete, and the policy still readable afterwards.
