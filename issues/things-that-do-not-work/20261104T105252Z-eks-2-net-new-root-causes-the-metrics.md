Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.addon-configuration-values-reach-the-in-cluster-workload eks.cluster-update-serialization eks.deploy-eks-cluster-cloudformation-active eks.nodegroup-full-lifecycle-transitions eks.nodegroup-labels-taints-round-trip eks.nodegroup-timestamps-precision-preserved eks.nodegroup-update-config-lifecycle eks.certificate-authority-premature-activation-rejected eks.e2e-certificate-authority-activation-to-teardown eks.e2e-certificate-authority-issue-activate-disable-delete eks.e2e-addon-config-update-consequence-flow eks.identity-provider-config-details-round-trip eks.e2e-cluster-nodegroup-workload-scale-teardown eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied eks.hpa-min-replicas-above-max-replicas-rejected eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.addon-configuration-values-reach-the-in-cluster-workload|_replica_property#11|AssertionError|assert len(candidates) == 1, (
Issue-Key: eks.cluster-update-serialization|_wait_for#29|Timeout|time.sleep(interval_s)
Issue-Key: eks.deploy-eks-cluster-cloudformation-active|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: eks.nodegroup-full-lifecycle-transitions|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: eks.nodegroup-labels-taints-round-trip|_wait_for_nodegroup_update#16|Timeout|time.sleep(interval_s)
Issue-Key: eks.nodegroup-timestamps-precision-preserved|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: eks.nodegroup-update-config-lifecycle|_wait_for#17|Timeout|time.sleep(interval_s)
Issue-Key: eks.certificate-authority-premature-activation-rejected|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: eks.e2e-certificate-authority-activation-to-teardown|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: eks.e2e-certificate-authority-issue-activate-disable-delete|_wait_ca_field#21|Timeout|time.sleep(interval_s)
Issue-Key: eks.e2e-addon-config-update-consequence-flow|_wait_addon_status#18|Timeout|time.sleep(interval_s)
Issue-Key: eks.identity-provider-config-details-round-trip|_wait_for_update#19|Timeout|time.sleep(interval_s)
Issue-Key: eks.e2e-cluster-nodegroup-workload-scale-teardown|_wait_update_successful#15|Timeout|time.sleep(interval_s)
Issue-Key: eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied|_wait_for#5|Timeout|time.sleep(interval_s)
Issue-Key: eks.hpa-min-replicas-above-max-replicas-rejected|_wait_for#3|Timeout|time.sleep(interval_s)
Issue-Key: eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops|_wait_for#3|Timeout|time.sleep(interval_s)
Issue-Key: eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling|_wait_for#3|Timeout|time.sleep(interval_s)
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks: 2 net-new root causes — the metrics-server add-on configuration schema declares no typed replica property, and asynchronous EKS resources never reach a terminal state (16 checks exhaust their budget)

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonConfiguration

## Description
Two net-new root causes left after the refusal-wording, over-permissive and data-plane clusters are accounted for.

Cause 1 of 2: the metrics-server add-on's configurationSchema declares a replica-count property with no integer type and no default.
  AWS: DescribeAddonConfiguration returns the add-on's JSON Schema for configurationValues. For metrics-server the schema declares exactly one integer replica-count property WITH a default, so a client can read the shipped replica count out of the schema and validate a configurationValues document against it before installing. (Recorded behavior for the exact property set; the Smithy model types configurationSchema only as a string.)
  Emulator: the schema's top-level properties are ['affinity', 'args', 'imagePullSecrets', 'namespace', 'nodeSelector', 'podAnnotations', 'podDisruptionBudget', 'podLabels', 'priorityClassName', 'replicas', 'resources', 'tolerations', 'topologySpreadConstraints'] — 'replicas' is present but no top-level property is an integer with a default, so the set of integer-with-default candidates is empty.
  Why: the schema is what a client validates against and what tooling reads defaults from; a 'replicas' property with no declared type/default means a valid document can be rejected locally (or an invalid one accepted) and the add-on's shipped replica count cannot be discovered. Related open issues 20261103T041817Z-eks-4-net-new-root-causes-in-the.md and 20261104T053622Z-eks-responses-omit-fields-aws-returns-cluster-vendedlogs.md name other defects of this catalogue; this check is reported unowned.
  Fix: the add-on catalogue entry that backs DescribeAddonConfiguration for metrics-server — the schema's replica property must carry type integer and its default.
  Traces: eks.addon-configuration-values-reach-the-in-cluster-workload.

Cause 2 of 2: asynchronous EKS resources never reach the terminal state their checks wait for.
  AWS: every create/update in these flows settles: a cluster reaches ACTIVE, a nodegroup reaches ACTIVE and its updates reach Successful, a certificate authority completes distribution and becomes activatable, an add-on and an identity-provider config reach ACTIVE, and a deleted resource stops describing. Each check then asserts the settled representation (labels/taints round-trip, timestamp precision, update serialization, CA activation ordering, add-on re-configuration).
  Emulator: sixteen checks make no assertion at all — they exhaust the suite's 60-second budget while polling, and the suite records 'Check timed out after 60.0s'. Hypothesis: the resources these flows poll do not transition (or transition far more slowly than AWS), so the state machines behind cluster/nodegroup/certificate-authority/add-on/identity-provider lifecycles never hand the caller a terminal state. The checks' own deadlines are minutes long, so nothing in them expects a 60-second settle.
  Why: a client that waits for ACTIVE/Successful — which is the normal way to use every one of these APIs — waits forever, so no workflow built on these resources can complete against the emulator. Related open issues 20261029T175608Z-eks-3-net-new-root-causes-startinsightsrefresh-response.md ('asynchronous EKS state that never settles') and 20261102T083656Z-eks-add-on-lifecycle-is-unreachable-clusters-never.md describe the same family for other operations; these sixteen checks are reported unowned.
  Fix: the lifecycle/state-transition machinery behind cluster, nodegroup, certificate-authority, add-on and identity-provider-config records — each must progress to its terminal state and record the update as Successful.
  Traces: eks.cluster-update-serialization, eks.deploy-eks-cluster-cloudformation-active, eks.nodegroup-full-lifecycle-transitions, eks.nodegroup-labels-taints-round-trip, eks.nodegroup-timestamps-precision-preserved, eks.nodegroup-update-config-lifecycle, eks.certificate-authority-premature-activation-rejected, eks.e2e-certificate-authority-activation-to-teardown, eks.e2e-certificate-authority-issue-activate-disable-delete, eks.e2e-addon-config-update-consequence-flow, eks.identity-provider-config-details-round-trip, eks.e2e-cluster-nodegroup-workload-scale-teardown, eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied, eks.hpa-min-replicas-above-max-replicas-rejected, eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops, eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service eks 142/306 passed).

Cause 1: eks.addon-configuration-values-reach-the-in-cluster-workload — 'expected exactly ONE integer replica-count property with a default in the metrics-server configuration schema; found [] among top-level properties [affinity, args, imagePullSecrets, namespace, nodeSelector, podAnnotations, podDisruptionBudget, podLabels, priorityClassName, replicas, resources, tolerations, topologySpreadConstraints]'.

Cause 2: sixteen checks report exactly 'Error: Check timed out after 60.0s' and no assertion failure — eks.cluster-update-serialization, eks.deploy-eks-cluster-cloudformation-active, eks.nodegroup-full-lifecycle-transitions, eks.nodegroup-labels-taints-round-trip, eks.nodegroup-timestamps-precision-preserved, eks.nodegroup-update-config-lifecycle, eks.certificate-authority-premature-activation-rejected, eks.e2e-certificate-authority-activation-to-teardown, eks.e2e-certificate-authority-issue-activate-disable-delete, eks.e2e-addon-config-update-consequence-flow, eks.identity-provider-config-details-round-trip, eks.e2e-cluster-nodegroup-workload-scale-teardown, eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied, eks.hpa-min-replicas-above-max-replicas-rejected, eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops, eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling.
Corroborating evidence from a check that did NOT time out: eks.addon-schema-invalid-configuration-rejected-before-conflict shows the emulator answering CreateAddon with 'Cluster <name> is in CREATING status; it must be ACTIVE to create an add-on.', and eks.ekscc-update-cluster-config-while-creating-rejected shows UpdateClusterConfig answering 'Cluster <name> is in CREATING status' — i.e. clusters are observed sitting in CREATING. The timed-out checks' own deadlines are minutes (e.g. _CLUSTER_DEADLINE_S = 1500.0, update polls with timeout_s=600), so the 60s suite budget is only the symptom's bound, not the expectation.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DescribeAddonConfiguration returns the add-on's configurationSchema as a string (the JSON Schema document itself is not modeled, so its content is recorded behavior); Nodegroup.status -> NodegroupStatus; Update/DescribeUpdate carry the update status.

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the check ids in trace_ids.
3. Manually, against the emulator's EKS endpoint:
   a. DescribeAddonConfiguration for metrics-server and parse configurationSchema -> expect exactly one top-level integer replica-count property carrying a default.
   b. CreateCluster, then poll DescribeCluster -> expect status ACTIVE; CreateNodegroup, then poll DescribeNodegroup -> expect ACTIVE; UpdateNodegroupConfig, then poll DescribeUpdate -> expect status Successful; CreateCertificateAuthority -> expect distribution to complete so ActivateCertificateAuthority is accepted; AssociateIdentityProviderConfig -> expect the config to reach ACTIVE. Each of these currently never settles, so any client that waits blocks indefinitely.
