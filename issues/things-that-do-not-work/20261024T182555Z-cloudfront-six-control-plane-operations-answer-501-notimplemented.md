Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.delete-resource-policy-in-use-illegal-delete-rejected cloudfront.get-resource-policy-invalid-argument-rejected cloudfront.op-contract-getResourcePolicy cloudfront.concurrent-update-origin-request-policy-single-winner cloudfront.update-origin-request-policy-inconsistent-quantities-fails cloudfront.update-origin-request-policy-inconsistent-quantities-rejected cloudfront.distribution-tenant-lifecycle cloudfront.op-contract-deleteContinuousDeploymentPolicy cloudfront.public-key-immutable-field-mutation-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.delete-resource-policy-in-use-illegal-delete-rejected|run#10|AssertionError|assert idle_code == "InvalidArgument", f"expected InvalidArgument on an idle connection group, got {idle_code!r}"
Issue-Key: cloudfront.get-resource-policy-invalid-argument-rejected|run#4|AssertionError|assert malformed_code == "InvalidArgument", (
Issue-Key: cloudfront.op-contract-getResourcePolicy|run#4|AssertionError|assert malformed_code == "InvalidArgument", (
Issue-Key: cloudfront.concurrent-update-origin-request-policy-single-winner|run#12|AssertionError|assert len(oks) == 1, f"expected exactly one winner, got {results}"
Issue-Key: cloudfront.update-origin-request-policy-inconsistent-quantities-fails|run#10|AssertionError|assert code == "InconsistentQuantities", f"expected InconsistentQuantities, got {code}"
Issue-Key: cloudfront.update-origin-request-policy-inconsistent-quantities-rejected|run#9|AssertionError|assert code == "InconsistentQuantities", code
Issue-Key: cloudfront.distribution-tenant-lifecycle|run#8|AssertionError|assert by_domain_code == "EntityNotFound", (
Issue-Key: cloudfront.op-contract-deleteContinuousDeploymentPolicy|run#17|AssertionError|assert unknown_code == "InvalidIfMatchVersion", (
Issue-Key: cloudfront.public-key-immutable-field-mutation-rejected|run#15|AssertionError|assert code == "CannotChangeImmutablePublicKeyFields", f"expected CannotChangeImmutablePublicKeyFields, got {code}"
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudfront: six control-plane operations answer 501 NotImplemented (PutResourcePolicy, GetResourcePolicy, UpdateOriginRequestPolicy, GetDistributionTenantByDomain, DeleteContinuousDeploymentPolicy, UpdatePublicKey)

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginRequestPolicy

## Description
This issue bundles SIX net-new root causes, one per operation. Every one of them is a CloudFront operation that is bound to the service in the Smithy model but whose handler is a scaffold: it answers HTTP 501 with error code `NotImplemented` and the message "operation <Name> is scaffolded but not implemented in this emulator" for every input, valid or invalid. A fixer must close all six before submitting.

The open issue 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md describes the same family for a DIFFERENT set of operations and does not name any of the six below; 20261023T140640Z-cloudfront-getresourcepolicy-stub.md names cause 2 only. None of the six checks listed here is marked ALREADY FILED or OWNED in the replay report, so this issue carries their replay keys.

Cause 1 of 6: PutResourcePolicy.
AWS: Smithy: PutResourcePolicy, POST /2020-05-31/put-resource-policy, success 200; modeled errors include InvalidArgument (400), EntityNotFound (404), UnsupportedOperation (400). Recorded behavior: a PutResourcePolicy naming a CONNECTION GROUP ARN is refused 400 InvalidArgument — the resource-type gate — with message "The specified resource type isn't valid. Specify a VPC origin and try again.", identically whether the group is idle or in use by a live tenant; a VPC-origin ARN in the same position is accepted and round-trips through GetResourcePolicy.
Emulator: 501 NotImplemented for the whole operation, so the resource-type gate cannot be observed at all.
Why: a caller that scopes a resource policy (RAM-style cross-account grants over VPC origins) cannot attach one, and a caller that probes which resource types are supported sees a server-side 501 instead of the modeled 400 it must branch on.
Fix: the PutResourcePolicy handler (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/put_resource_policy.rs) plus the resource-policy slice of state.
Traces: cloudfront.delete-resource-policy-in-use-illegal-delete-rejected.

Cause 2 of 6: GetResourcePolicy.
AWS: Smithy: GetResourcePolicy, POST /2020-05-31/get-resource-policy, success 200; modeled errors AccessDenied (403), EntityNotFound (404), InvalidArgument (400), UnsupportedOperation (400). Recorded behavior: a malformed/unsupported ResourceArn is refused 400 InvalidArgument.
Emulator: 501 NotImplemented for every call, including the malformed-ARN case.
Why: no caller can read back a resource policy, and the malformed-input contract is unobservable.
Fix: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_resource_policy.rs. Already named by the open issue 20261023T140640Z-cloudfront-getresourcepolicy-stub.md — close them together.
Traces: cloudfront.get-resource-policy-invalid-argument-rejected, cloudfront.op-contract-getResourcePolicy.

Cause 3 of 6: UpdateOriginRequestPolicy.
AWS: Smithy: UpdateOriginRequestPolicy, PUT /2020-05-31/origin-request-policy/{Id}, success 200, If-Match header; modeled errors include InconsistentQuantities (400, "The value of Quantity and the size of Items don't match."), InvalidIfMatchVersion (400), PreconditionFailed (412), NoSuchOriginRequestPolicy (404). Two concurrent updates from the same ETag must resolve to exactly one winner, the loser refused on the version precondition.
Emulator: every update answers 501 NotImplemented, so there is no winner at all (both racers report NotImplemented) and a Quantity/Items mismatch is never refused with InconsistentQuantities.
Why: a policy can be created but never changed; optimistic-concurrency clients get a 501 where they expect either 200 or a 400/412 precondition refusal.
Fix: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_origin_request_policy.rs, including the If-Match/ETag check and the quantity-consistency validation.
Traces: cloudfront.concurrent-update-origin-request-policy-single-winner, cloudfront.update-origin-request-policy-inconsistent-quantities-fails, cloudfront.update-origin-request-policy-inconsistent-quantities-rejected.

Cause 4 of 6: GetDistributionTenantByDomain.
AWS: Smithy: GetDistributionTenantByDomain, GET /2020-05-31/distribution-tenant?domain=..., success 200; modeled errors AccessDenied (403) and EntityNotFound (404). A domain that is not yet resolvable to a tenant (its managed certificate request still pending validation) answers 404 EntityNotFound.
Emulator: 501 NotImplemented.
Why: the by-domain lookup — the only way a caller maps an incoming host to a tenant — is unavailable, and the pending-domain case cannot be distinguished from a server fault.
Fix: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_distribution_tenant_by_domain.rs.
Traces: cloudfront.distribution-tenant-lifecycle.

Cause 5 of 6: DeleteContinuousDeploymentPolicy.
AWS: Smithy: DeleteContinuousDeploymentPolicy, DELETE /2020-05-31/continuous-deployment-policy/{Id}, success 204; modeled errors include InvalidIfMatchVersion (400), NoSuchContinuousDeploymentPolicy (404), ContinuousDeploymentPolicyInUse (409), PreconditionFailed (412). A delete of an unknown Id with a bogus If-Match is refused 400 InvalidIfMatchVersion.
Emulator: 501 NotImplemented.
Why: a continuous-deployment policy can never be removed, so the staging-distribution lifecycle cannot be torn down and the modeled refusal classes are unobservable.
Fix: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_continuous_deployment_policy.rs.
Traces: cloudfront.op-contract-deleteContinuousDeploymentPolicy.

Cause 6 of 6: UpdatePublicKey.
AWS: Smithy: UpdatePublicKey, PUT /2020-05-31/public-key/{Id}/config, success 200; modeled errors include CannotChangeImmutablePublicKeyFields (400, "You can't change the value of a public key."), IllegalUpdate (400), InvalidIfMatchVersion (400), PreconditionFailed (412). Changing EncodedKey is refused 400 CannotChangeImmutablePublicKeyFields and leaves the record and its ETag untouched; a Comment-only update on the same key succeeds.
Emulator: 501 NotImplemented for both the refused and the legal update.
Why: signed-URL/signed-cookie key rotation workflows cannot update a public key's comment, and the immutability contract they rely on cannot be exercised.
Fix: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_public_key.rs, including the immutable-field comparison against the stored config.
Traces: cloudfront.public-key-immutable-field-mutation-rejected.

## Evidence
Replay output, verbatim tails:
- cloudfront.delete-resource-policy-in-use-illegal-delete-rejected: AssertionError: expected InvalidArgument on an idle connection group, got 'NotImplemented' (call: cf.put_resource_policy(ResourceArn=<connection-group ARN>, PolicyDocument=...); expected message "The specified resource type isn't valid. Specify a VPC origin and try again.").
- cloudfront.get-resource-policy-invalid-argument-rejected: AssertionError: expected InvalidArgument for malformed ARN, got 'NotImplemented'.
- cloudfront.op-contract-getResourcePolicy: AssertionError: expected InvalidArgument for a malformed ResourceArn, got NotImplemented.
- cloudfront.concurrent-update-origin-request-policy-single-winner: AssertionError: expected exactly one winner, got [('a', 'NotImplemented', None), ('b', 'NotImplemented', None)].
- cloudfront.update-origin-request-policy-inconsistent-quantities-fails: AssertionError: expected InconsistentQuantities, got NotImplemented.
- cloudfront.update-origin-request-policy-inconsistent-quantities-rejected: AssertionError: NotImplemented.
- cloudfront.distribution-tenant-lifecycle: AssertionError: expected GetDistributionTenantByDomain to report EntityNotFound for a domain whose managed certificate is still pending validation, got 'NotImplemented'.
- cloudfront.op-contract-deleteContinuousDeploymentPolicy: AssertionError: expected InvalidIfMatchVersion for an unknown Id, got NotImplemented.
- cloudfront.public-key-immutable-field-mutation-rejected: AssertionError: expected CannotChangeImmutablePublicKeyFields, got NotImplemented.

Expected (AWS): the modeled 4xx refusal or the 200/204 success listed per cause above. Actual (emulator): HTTP 501, Code=NotImplemented, Message="operation <Name> is scaffolded but not implemented in this emulator".

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudfront client at it.
2. PutResourcePolicy/GetResourcePolicy with any ResourceArn -> 501 NotImplemented.
3. CreateOriginRequestPolicy, then UpdateOriginRequestPolicy with its ETag -> 501 NotImplemented (run two concurrently: neither wins).
4. GetDistributionTenantByDomain(Domain=<any>) -> 501 NotImplemented.
5. DeleteContinuousDeploymentPolicy(Id=<unknown>, IfMatch=<bogus>) -> 501 NotImplemented instead of 400 InvalidIfMatchVersion.
6. CreatePublicKey, then UpdatePublicKey changing EncodedKey -> 501 NotImplemented instead of 400 CannotChangeImmutablePublicKeyFields.
Or replay the nine check ids in trace_ids.
