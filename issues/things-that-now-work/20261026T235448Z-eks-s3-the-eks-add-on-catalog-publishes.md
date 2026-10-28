Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle
Issue-Key: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle|phase#8|_PhaseError|raise _PhaseError(f"[phase {i}/{len(self.phases)} {name}] {type(exc).__name__}: {exc}") from exc
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# eks__s3: the EKS add-on catalog publishes neither cert-manager nor amazon-sagemaker-hyperpod-training-operator, so the EKS+S3 lifecycle workflow aborts in its first phase

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
One root cause.

Cause 1 of 1: the seeded EKS add-on catalog does not publish cert-manager or amazon-sagemaker-hyperpod-training-operator, so DescribeAddonVersions reports an empty listing for both and no cluster version compatible with them can be resolved.
AWS: Smithy: eks#DescribeAddonVersions, GET /addons/supported-versions, input addonName (httpQuery, String) 'The name of the add-on. The name must match one of the names returned by ListAddons', output addons -> Addons (a list of AddonInfo), errors InvalidParameterException/400, ResourceNotFoundException/404, ServerException/500. The operation reports the EKS add-on catalog, a publication rather than per-account data: for an add-on AWS publishes it returns exactly one AddonInfo, carrying defaultNamespace and addonVersions whose compatibilities name the Kubernetes cluster versions each build supports and flag one of them defaultVersion. cert-manager and amazon-sagemaker-hyperpod-training-operator are both published EKS add-ons, so DescribeAddonVersions(addonName=<either>) — with or without kubernetesVersion — returns a one-element addons list.
Emulator: both names answer HTTP 200 with addons: [], so indexing the first AddonInfo raises IndexError and the workflow's deploy phase aborts before any cluster, add-on or S3 object exists. The handler itself is right to treat an unpublished name as an empty listing rather than an error (its module doc says so); the defect is the catalog SEED, which does not publish these add-ons.
Why: a client cannot discover any version of these add-ons, cannot choose a Kubernetes version compatible with them, and cannot install them — CreateAddon for a name the catalog does not publish is refused InvalidParameterException 'Addon <name> specified is not supported in <version> kubernetes version'. Because the catalog is the entry point of the whole workflow, everything downstream is unobservable too: the baseline add-ons on a fresh cluster, where each task add-on settles on a nodeless cluster (cert-manager DEGRADED with health issue InsufficientNumberOfReplicas; amazon-sagemaker-hyperpod-training-operator CREATE_FAILED with AdmissionRequestDenied), and the S3 legs that follow (ListObjectVersions/DeleteObjects over the versioned service buckets).
Fix: the add-on catalog seed in this service's state (crate::state::AddonCatalogRecord, read by published_addons) must publish the add-ons Amazon EKS publishes — including cert-manager and amazon-sagemaker-hyperpod-training-operator — with their builds, defaultNamespace, publisher/owner/type and per-build compatibilities including the defaultVersion flag.
Same root cause as, but not covered by, the eks-token issue filed in this same sweep ('eks: 3 net-new root causes — the add-on catalog omits cert-manager and the HyperPod training operator, ...'): that issue carries only the eks.* replay key, so this check must be re-replayed against the catalog fix as well.
Traces: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle.

## Evidence
Suite simparity-behavioral-ga-prod-v1; eks__s3 0/2, this check not marked ALREADY FILED or OWNED.

eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle [error]:
  _PhaseError: [phase 1/7 deploy] IndexError: list index out of range
  raised from check_eks__s3_e2e_manage_buckets_eks_sagemaker_lifecycle.py line 4046 in _cluster_version:
    entry = eks.describe_addon_versions(addonName=addon)['addons'][0]
  with addon iterating ('cert-manager', 'amazon-sagemaker-hyperpod-training-operator').

Expected: each of the two calls returns addons with exactly one AddonInfo, whose addonVersions[].compatibilities[].clusterVersion set intersects the STANDARD_SUPPORT versions reported by DescribeClusterVersions, so the newest version both add-ons support can be chosen; and DescribeAddonVersions(addonName=<either>, kubernetesVersion=<that version>) returns that add-on with exactly one build flagged defaultVersion for it plus the add-on's defaultNamespace.
Actual: HTTP 200 with addons: [] for both names. DescribeClusterVersions itself answered (the STANDARD_SUPPORT list was built before the failing line), so the gap is confined to the add-on catalog.

Never reached because of this cause (what the fix must unblock, same single cause): a fresh cluster lists the baseline add-ons; CreateAddon('cert-manager') settles DEGRADED with health issue code InsufficientNumberOfReplicas on a nodeless cluster, and CreateAddon('amazon-sagemaker-hyperpod-training-operator') installed after it settles CREATE_FAILED with health issue code AdmissionRequestDenied — read from DescribeAddon after the status leaves CREATING, never from the create call; then the S3 legs over the six versioned service buckets (ListObjectVersions, then DeleteObjects of every Version and DeleteMarker).

## Steps to Reproduce
Boot the emulator and point boto3 at it.

1. eks.describe_addon_versions(addonName='cert-manager') -> expect one AddonInfo with defaultNamespace and addonVersions[].compatibilities; observe HTTP 200 with addons: [].
2. eks.describe_addon_versions(addonName='amazon-sagemaker-hyperpod-training-operator') -> same empty listing.
3. Intersect the compatibilities of the two listings with the STANDARD_SUPPORT versions from eks.describe_cluster_versions() -> on AWS this yields a usable cluster version; here step 1 already fails.
4. (Blocked by 1-2) create a VPC with two EKS-supported AZ subnets, a cluster role, and a nodeless cluster on that version; install cert-manager and wait for it to leave CREATING (expect DEGRADED / InsufficientNumberOfReplicas), then install amazon-sagemaker-hyperpod-training-operator and wait (expect CREATE_FAILED / AdmissionRequestDenied); then run ListObjectVersions + DeleteObjects over the versioned service buckets.
   Replay: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle
