Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.capability-ops-on-missing-cluster-not-found eks.create-capability-argocd-config-on-ack-type-rejected eks.certificate-authority-unknown-not-found eks.describe-certificate-authority-unknown-not-found eks.cluster-apis-unknown-cluster-not-found eks.op-update-cluster-config-validation eks.create-eks-anywhere-subscription-malformed-license-rejected eks.create-nodegroup-disk-size-out-of-range-rejected eks.describe-insights-refresh-unknown-cluster-not-found eks.disassociate-identity-provider-config-not-found eks.op-create-children-require-existing-cluster eks.op-mutations-require-existing-cluster eks.op-cross-account-pass-role-denied
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.capability-ops-on-missing-cluster-not-found|run#7|AssertionError|assert not mismatches, f"expected ResourceNotFoundException everywhere, got {mismatches}"
Issue-Key: eks.create-capability-argocd-config-on-ack-type-rejected|run#8|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: eks.certificate-authority-unknown-not-found|run#6|AssertionError|assert describe_code == "ResourceNotFoundException", describe_code
Issue-Key: eks.describe-certificate-authority-unknown-not-found|run#6|AssertionError|assert describe_code == "ResourceNotFoundException", (
Issue-Key: eks.cluster-apis-unknown-cluster-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: eks.op-update-cluster-config-validation|run#4|AssertionError|assert code == "ResourceNotFoundException", f"bad log type -> {code}: {msg}"
Issue-Key: eks.create-eks-anywhere-subscription-malformed-license-rejected|run#4|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: eks.create-nodegroup-disk-size-out-of-range-rejected|run#8|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: eks.describe-insights-refresh-unknown-cluster-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: eks.disassociate-identity-provider-config-not-found|run#5|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: eks.op-create-children-require-existing-cluster|run#6|AssertionError|assert code == "ResourceNotFoundException", f"{op} -> {code}"
Issue-Key: eks.op-mutations-require-existing-cluster|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: eks.op-cross-account-pass-role-denied|run#11|AssertionError|assert code == "AccessDeniedException", f"{op} -> {code}: {msg}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EKS control-plane operations answer 501 NotImplemented or validate in the wrong order (10 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateClusterConfig

## Description
Replaying the behavioral suite against the EKS emulator (service crate services/service-com-amazonaws-eks-awswesleyfrontend) shows TEN distinct root causes in the control-plane surface: nine operations (or operation families) are scaffold stubs that answer HTTP 501 `NotImplemented` where AWS answers a modeled result or a modeled 4xx refusal, and one operation validates the request body before it authorizes iam:PassRole. Each cause below must be closed; the issue is NOT complete until all ten are.

Cause 1 of 10 — the EKS *capability* APIs are stubs.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): DescribeCapability / UpdateCapability / DeleteCapability each model ResourceNotFoundException (httpError 404) and InvalidParameterException (400); CreateCapability models InvalidParameterException (400) and InvalidRequestException (400). ListCapabilities models only InvalidParameterException/ServerException, so its not-found behavior is recorded behavior only: on a cluster that does not exist the recording shows error code ResourceNotFoundException. A CreateCapability carrying an argocd configuration on an `ack`-typed capability is refused with InvalidParameterException.
Emulator: every one of ListCapabilities, DescribeCapability, UpdateCapability, DeleteCapability, CreateCapability answers code `NotImplemented` (message example: "operation DeleteCapability is scaffolded but not implemented in this emulator").
Why: a client cannot distinguish "cluster absent" from "bad argument" from "service broken"; SDK retry/branching keyed on ResourceNotFoundException or InvalidParameterException never fires, and capability provisioning code paths cannot be exercised at all.
Fix: the capability operation handlers and their cluster-resolution/validation step in the eks crate's ops module.
Traces: eks.capability-ops-on-missing-cluster-not-found, eks.create-capability-argocd-config-on-ack-type-rejected.

Cause 2 of 10 — cluster certificate-authority read APIs are stubs.
AWS: Smithy: DescribeCertificateAuthority and ListCertificateAuthorities both model ResourceNotFoundException (httpError 404). The refusal message is built from the request (the cluster name named in the call), recorded as "No cluster found for name: <clusterName>." — that construction rule is the requirement, not the literal recorded name.
Emulator: both answer `NotImplemented`.
Why: callers probing for a cluster's CA set get an unmodeled error class and cannot treat the cluster as absent.
Fix: the DescribeCertificateAuthority / ListCertificateAuthorities handlers, resolving clusterName first.
Traces: eks.certificate-authority-unknown-not-found, eks.describe-certificate-authority-unknown-not-found.

Cause 3 of 10 — UpdateClusterConfig is a stub, and the generic body validator runs ahead of cluster resolution.
AWS: Smithy: UpdateClusterConfig models ResourceNotFoundException (404) and InvalidParameterException (400); the recording shows the cluster is resolved FIRST — an unknown cluster is answered 404 ResourceNotFoundException "No cluster found for name: <name>." even when the logging body carries an unknown `types` enum value.
Emulator: with a well-formed body on an unknown cluster it answers `NotImplemented`; with an invalid log type on the same unknown cluster it answers 400 InvalidParameterException ("1 validation error detected: Value 'definitely-not-a-log-type' at 'logging.clusterLogging.1.member.types.1.member' failed to satisfy constraint: Member must satisfy enum value set: [api, audit, authenticator, controllerManager, scheduler]"). So the order is inverted and the implemented path is only the validator.
Why: a client that keys on ResourceNotFoundException to create-then-configure a cluster sees a 400 validation error (or a 501) instead, and never learns the cluster is missing.
Fix: the UpdateClusterConfig handler plus the ordering between the shared request validator and cluster resolution.
Traces: eks.cluster-apis-unknown-cluster-not-found (UpdateClusterConfig leg; DescribeCluster/DeleteCluster legs of that check already behave), eks.op-update-cluster-config-validation.

Cause 4 of 10 — CreateEksAnywhereSubscription is a stub.
AWS: Smithy: CreateEksAnywhereSubscription models InvalidParameterException (400); `term` is documented to accept duration 12 or 36 with unit MONTHS, and a duration of 5 is refused with InvalidParameterException (recorded message example: "term duration not one of: 12,36"), with no subscription created under the attempted name.
Emulator: answers `NotImplemented`.
Why: an out-of-contract term is neither accepted nor properly refused, so a client cannot validate subscription input against the emulator.
Fix: the CreateEksAnywhereSubscription handler and its term validation.
Traces: eks.create-eks-anywhere-subscription-malformed-license-rejected.

Cause 5 of 10 — CreateNodegroup is a stub.
AWS: Smithy: CreateNodegroup models InvalidParameterException (400) and returns `nodegroup` with status CREATING on success; `diskSize` is a BoxedInteger and a negative value (-1) is refused with InvalidParameterException, with the nodegroup absent from ListNodegroups afterwards.
Emulator: answers `NotImplemented` (the cluster itself was created successfully by the same check, so this is the nodegroup handler alone).
Why: neither the refusal nor the happy path of managed node groups is observable; every nodegroup-dependent client flow is blocked.
Fix: the CreateNodegroup handler, its diskSize/scalingConfig validation and nodegroup state.
Traces: eks.create-nodegroup-disk-size-out-of-range-rejected.

Cause 6 of 10 — DescribeInsightsRefresh is a stub.
AWS: Smithy: DescribeInsightsRefresh models ResourceNotFoundException (404); on a cluster that does not exist the refusal message is built from the request ("No cluster found for name: <clusterName>.").
Emulator: answers `NotImplemented`.
Why: insight-refresh polling cannot distinguish a missing cluster from a broken service.
Fix: the DescribeInsightsRefresh handler, resolving clusterName first.
Traces: eks.describe-insights-refresh-unknown-cluster-not-found.

Cause 7 of 10 — DisassociateIdentityProviderConfig is a stub.
AWS: Smithy: DisassociateIdentityProviderConfig models InvalidParameterException (400) and ResourceNotFoundException (404). The recording shows two distinct paths on the same made-up cluster: an unsupported identityProviderConfig.type is refused with InvalidParameterException (type validation), while the one valid type "oidc" takes the cluster-resolution path and is refused with ResourceNotFoundException.
Emulator: both calls answer `NotImplemented`, so neither path exists.
Why: an emulator that neither rejects an unsupported provider type nor reports the missing cluster lets a client's IdP teardown logic pass on input AWS refuses.
Fix: the DisassociateIdentityProviderConfig handler: validate identityProviderConfig.type, then resolve the cluster.
Traces: eks.disassociate-identity-provider-config-not-found.

Cause 8 of 10 — CreateAddon against a nonexistent cluster answers 501 instead of 404.
AWS: Smithy: CreateAddon models ResourceNotFoundException (404) and InvalidParameterException (400); creating an addon on a cluster that was never created is a not-found.
Emulator: answers `NotImplemented`. NOTE: an open issue already names this operation and handler file (filed from eks.op-validation-precedence-is-layered); it does NOT carry this check's replay key, so this check is listed here and must be re-replayed when that handler lands.
Why: child-resource creates cannot report the absent parent, so a client's create-parent-then-child recovery never triggers.
Fix: the CreateAddon handler's cluster resolution.
Traces: eks.op-create-children-require-existing-cluster.

Cause 9 of 10 — UpdateNodegroupConfig is a stub.
AWS: Smithy: UpdateNodegroupConfig models ResourceNotFoundException (404) and InvalidParameterException (400); a mutation naming a cluster that does not exist is answered ResourceNotFoundException.
Emulator: answers `NotImplemented`.
Why: nodegroup mutations on a missing cluster/nodegroup are indistinguishable from a service fault.
Fix: the UpdateNodegroupConfig handler and its cluster/nodegroup resolution.
Traces: eks.op-mutations-require-existing-cluster.

Cause 10 of 10 — CreateCluster validates the body before authorizing iam:PassRole.
AWS: recorded behavior only (CreateCluster's modeled error list does not include AccessDeniedException): EKS authorizes the pass-role of `roleArn` BEFORE validating the rest of the request, so a roleArn in a different account is refused with error code AccessDeniedException and the fixed sentence "Cross-account pass role is not allowed." even when `name` violates the modeled pattern and the subnets do not exist; the same denial is returned for a syntactically malformed roleArn ("not-an-arn"). Smithy: CreateCluster.name is @length 1..100 @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ — that validation exists, it just must not pre-empt the pass-role check. The check's positive control (the identical malformed body with an in-account role) passed, so the denial is earned by the foreign account, not by the body.
Emulator: answers 400 InvalidParameterException "1 validation error detected: Value 'sim2real behav bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\-_]*$".
Why: a client (or a policy test) asserting that cross-account role passing is denied sees a parameter complaint instead, and an emulator-backed test suite would conclude cross-account PassRole is permitted.
Fix: the CreateCluster handler — move the roleArn pass-role authorization ahead of body validation (the same ordering question as cause 3).
Traces: eks.op-cross-account-pass-role-denied.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/306 passed; eks 0/279.

Cause 1: eks.capability-ops-on-missing-cluster-not-found — AssertionError: expected ResourceNotFoundException everywhere, got {'ListCapabilities': 'NotImplemented', 'DescribeCapability': 'NotImplemented', 'UpdateCapability': 'NotImplemented', 'DeleteCapability': 'NotImplemented'}. eks.create-capability-argocd-config-on-ack-type-rejected — AssertionError: expected InvalidParameterException, got NotImplemented.
Cause 2: eks.certificate-authority-unknown-not-found — `assert describe_code == "ResourceNotFoundException"` -> AssertionError: NotImplemented. eks.describe-certificate-authority-unknown-not-found — AssertionError: expected ResourceNotFoundException from DescribeCertificateAuthority, got 'NotImplemented'.
Cause 3: eks.cluster-apis-unknown-cluster-not-found — AssertionError: UpdateClusterConfig on unknown cluster: expected ResourceNotFoundException, got 'NotImplemented'. eks.op-update-cluster-config-validation — AssertionError: bad log type -> InvalidParameterException: 1 validation error detected: Value 'definitely-not-a-log-type' at 'logging.clusterLogging.1.member.types.1.member' failed to satisfy constraint: Member must satisfy enum value set: [api, audit, authenticator, controllerManager, scheduler]  (expected ResourceNotFoundException "No cluster found for name: <ghost>.").
Cause 4: eks.create-eks-anywhere-subscription-malformed-license-rejected — AssertionError: expected InvalidParameterException, got 'NotImplemented' (term {duration:5, unit:MONTHS}).
Cause 5: eks.create-nodegroup-disk-size-out-of-range-rejected — AssertionError: expected InvalidParameterException, got NotImplemented (diskSize=-1).
Cause 6: eks.describe-insights-refresh-unknown-cluster-not-found — AssertionError: expected ResourceNotFoundException, got 'NotImplemented'.
Cause 7: eks.disassociate-identity-provider-config-not-found — AssertionError: expected InvalidParameterException, got 'NotImplemented' (type "not-a-supported-type"); the "oidc" positive control was never reached.
Cause 8: eks.op-create-children-require-existing-cluster — AssertionError: CreateAddon -> NotImplemented (expected ResourceNotFoundException).
Cause 9: eks.op-mutations-require-existing-cluster — AssertionError: UpdateNodegroupConfig: expected ResourceNotFoundException, got 'NotImplemented'.
Cause 10: eks.op-cross-account-pass-role-denied — AssertionError: CreateCluster -> InvalidParameterException: 1 validation error detected: Value 'sim2real behav bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\-_]*$  (expected AccessDeniedException "Cross-account pass role is not allowed.").

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it.
2. Cause 1: eks list_capabilities / describe_capability / update_capability / delete_capability on a cluster name that was never created; and create_capability with type 'ack' plus an argocd configuration. Expect ResourceNotFoundException / InvalidParameterException; observe NotImplemented.
3. Cause 2: describe_certificate_authority(clusterName=<absent>, certificateAuthorityId='00000000-0000-0000-0000-000000000000') and list_certificate_authorities(clusterName=<absent>). Expect 404 ResourceNotFoundException with "No cluster found for name: <absent>.".
4. Cause 3: update_cluster_config(name=<absent>, logging={'clusterLogging':[{'types':['api'],'enabled':True}]}) -> expect 404 ResourceNotFoundException; then the same call with types=['definitely-not-a-log-type'] -> expect the SAME 404, not a 400 enum validation error.
5. Cause 4: create_eks_anywhere_subscription(name=<new>, term={'duration':5,'unit':'MONTHS'}) -> expect 400 InvalidParameterException and no subscription created.
6. Cause 5: create_nodegroup(..., diskSize=-1) on an ACTIVE cluster -> expect 400 InvalidParameterException; then the same call with diskSize=20 -> expect nodegroup status CREATING and the name present in list_nodegroups.
7. Cause 6: describe_insights_refresh(clusterName=<absent>) -> expect 404 ResourceNotFoundException.
8. Cause 7: disassociate_identity_provider_config(clusterName=<absent>, identityProviderConfig={'type':'not-a-supported-type','name':X}) -> expect 400 InvalidParameterException; with type 'oidc' -> expect 404 ResourceNotFoundException.
9. Cause 8: create_addon(clusterName=<absent>, addonName='vpc-cni') -> expect 404 ResourceNotFoundException.
10. Cause 9: update_nodegroup_config(clusterName=<absent>, nodegroupName=X, ...) -> expect 404 ResourceNotFoundException.
11. Cause 10: create_cluster(name='sim2real behav bad name!', roleArn='arn:aws:iam::111122223333:role/<any>', resourcesVpcConfig={'subnetIds':[...]}) -> expect AccessDeniedException "Cross-account pass role is not allowed."; repeat with roleArn='not-an-arn' -> same denial.
