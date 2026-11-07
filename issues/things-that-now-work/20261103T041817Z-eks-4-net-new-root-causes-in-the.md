Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.describe-addon-versions-multi-value-types-filter-is-a-union-narrowed-by-owners eks.efs-csi-driver-versions-listed-newest-first eks.mbes-addon-configuration-names-the-operator-pod-identity-policy eks.mbes-addon-versions-filter-by-cluster-version-is-a-subset
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.describe-addon-versions-multi-value-types-filter-is-a-union-narrowed-by-owners|run#14|AssertionError|assert owner and owner != catalog[_SECURITY].get("owner"), (
Issue-Key: eks.efs-csi-driver-versions-listed-newest-first|run#11|AssertionError|assert len(majors) >= 2, f"the list spans only major version(s) {sorted(majors)!r}"
Issue-Key: eks.mbes-addon-configuration-names-the-operator-pod-identity-policy|run#3|AssertionError|assert schema.get("$schema") == "http://json-schema.org/draft-07/schema#", (
Issue-Key: eks.mbes-addon-versions-filter-by-cluster-version-is-a-subset|run#8|AssertionError|assert only_a or only_b, (
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# eks: 4 net-new root causes in the add-on catalog's per-add-on metadata — owner, published version history, compatibilities and the configuration-schema dialect

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
FOUR net-new root causes in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend), all in the per-add-on metadata the add-on catalog projects through DescribeAddonVersions and DescribeAddonConfiguration. Each answers HTTP 200 with the right envelope but with metadata that is the same for every add-on instead of that add-on's own catalog record, so the catalog cannot be used to discriminate between add-ons. All four must be closed.

Related open issues, named here because they do NOT carry these behavioral check ids and do not cover these members: 20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md (wire corpus; its causes 1-4 cover invented version entries/owner/type/publisher for other add-ons) and 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md (wire corpus; missing Marketplace add-ons and a placeholder vpc-cni schema). A fixer closing those issues would still have to re-replay the checks listed per cause below.

Cause 1 of 4 — every add-on in the catalog is served with owner 'aws', so the `owners` filter cannot narrow anything.
AWS: Smithy: DescribeAddonVersions (GET /addons/supported-versions) returns addons: Addons of AddonInfo, and AddonInfo.owner is documented as "The owner of the add-on"; DescribeAddonVersions also takes an `owners` httpQuery filter, which only means anything if different add-ons carry different owners. Recorded behavior: the AWS catalog gives `amazon-sagemaker-hyperpod-training-operator` and `cert-manager` DIFFERENT owners — the AWS-published operator is owned by AWS, and cert-manager is not (it carries its own third-party/community owner token). The requirement is that AddonInfo.owner is that add-on's catalog owner, not a constant.
Emulator: DescribeAddonVersions reports owner 'aws' for BOTH add-ons, so `cert-manager` is advertised as an AWS-owned add-on.
Why: `owners` is a server-side filter a client uses to separate AWS add-ons from third-party/Marketplace ones (and to decide whether a subscription is needed). With one constant owner the filter is either a no-op or empties the catalog, and a client auditing which add-ons are vendor-supplied gets the wrong answer for every non-AWS add-on.
Fix: the add-on catalog data behind DescribeAddonVersions' handler (the per-add-on owner field) and the `owners` filter path that reads it.
Traces: eks.describe-addon-versions-multi-value-types-filter-is-a-union-narrowed-by-owners.

Cause 2 of 4 — the published version history of aws-efs-csi-driver covers only one major version.
AWS: Smithy: AddonInfo.addonVersions is AddonVersionInfoList and each AddonVersionInfo.addonVersion is "The version of the add-on". Recorded behavior: the catalog for `aws-efs-csi-driver` lists the add-on's published history, which spans more than one major version (majors 1 through 3 of the driver, newest first).
Emulator: `describe_addon_versions(addonName='aws-efs-csi-driver')` returns versions whose major component is 2 for every entry — the entries are unique, well formed as vX.Y.Z-eksbuild.N and correctly ordered newest-first (those assertions passed), but the majors outside 2 are absent from the seed.
Why: a client pinning, upgrading or rolling back to a specific driver version cannot find it in the catalog and concludes it was never published; a client that takes addonVersions[0] as "latest" gets a version that is not the newest AWS offers.
Fix: the add-on catalog data behind DescribeAddonVersions' handler (the version list seeded for aws-efs-csi-driver). The open wire issue 20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md names this add-on's NEWEST version only (its cause 4); the requirement here is the whole published history, so re-replay the check below after fixing it.
Traces: eks.efs-csi-driver-versions-listed-newest-first.

Cause 3 of 4 — compatibilities are not per add-on: unrelated add-ons advertise an identical compatible cluster-version set.
AWS: Smithy: AddonVersionInfo.compatibilities is Compatibilities of Compatibility {clusterVersion, platformVersions, defaultVersion}, and DescribeAddonVersions takes a `kubernetesVersion` httpQuery filter that selects the versions whose compatibilities name it. Recorded behavior: the compatible cluster-version set is a property of each add-on's own versions, so two unrelated add-ons (`cert-manager` and `amazon-sagemaker-hyperpod-training-operator`) do NOT name the same set; a cluster version real for one add-on but outside the other's set must empty the other add-on's filtered read (addons == []).
Emulator: both add-ons name exactly the same cluster-version set (every entry carries the same block of consecutive minors, e.g. 1.30 through 1.37), so there is no cluster version that is compatible for one and not the other; the compatibility block is synthesised from the cluster-version catalog at large rather than from the add-on's record.
Why: `kubernetesVersion` is the filter a client uses to answer "which versions of this add-on can I install on this cluster". Against the emulator every add-on appears installable on every supported minor, so a client installs an add-on version AWS refuses for that cluster, and an emulator-backed test concludes the compatibility matrix is enforced when it is not.
Fix: the add-on catalog data behind DescribeAddonVersions' handler — per-add-on, per-version compatibilities entries, and the kubernetesVersion filter that matches against THIS add-on's compatibilities rather than the service's cluster-version list.
Traces: eks.mbes-addon-versions-filter-by-cluster-version-is-a-subset.

Cause 4 of 4 — DescribeAddonConfiguration serves one JSON Schema dialect for every add-on, so the operator add-on's schema declares the wrong $schema.
AWS: Smithy: DescribeAddonConfiguration (GET /addons/configuration-schemas, addonName and addonVersion both required httpQuery) returns configurationSchema: String. Recorded behavior: the returned document is the add-on's OWN published JSON Schema, and the dialect is per add-on — the newest `amazon-sagemaker-hyperpod-training-operator` schema declares "$schema": "http://json-schema.org/draft-07/schema#" (fixed text; the value is the schema's dialect URI, not a per-run value), while other add-ons publish draft-06 documents.
Emulator: the schema returned for amazon-sagemaker-hyperpod-training-operator declares "$schema": "http://json-schema.org/draft-06/schema#" — the dialect is fixed service-wide instead of being taken from the add-on's document. The check stops there, so the rest of that add-on's configuration surface (requiresIamPermissions, podIdentityConfiguration naming the service account that recommends arn:aws:iam::aws:policy/AmazonSageMakerHyperPodTrainingOperatorAccess, and cert-manager's empty podIdentityConfiguration) was not reached and may hide further divergence in the same seed.
Why: a client that validates its configurationValues document against the advertised schema loads it with a draft-06 validator and silently ignores draft-07 keywords (if/then/else, const, writeOnly), so configuration AWS rejects is accepted locally — the schema is only useful if it is the add-on's actual document.
Fix: the per-add-on configuration-schema seed behind DescribeAddonConfiguration's handler (store and emit each add-on's own schema document, including its $schema dialect).
Traces: eks.mbes-addon-configuration-names-the-operator-pod-identity-policy.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 80/342 passed; eks 73/306. None of the four checks below is marked ALREADY FILED or OWNED in the report.

Cause 1: eks.describe-addon-versions-multi-value-types-filter-is-a-union-narrowed-by-owners — [mismatch] AssertionError: "the two add-ons must have distinct owners; got 'aws' and 'aws'" (at `assert owner and owner != catalog[_SECURITY].get("owner")`, after the types-union assertions for amazon-sagemaker-hyperpod-training-operator and cert-manager passed).
Cause 2: eks.efs-csi-driver-versions-listed-newest-first — [mismatch] AssertionError: "the list spans only major version(s) [2]" (uniqueness, the vX.Y.Z-eksbuild.N form and the strict numeric newest-first ordering all passed first).
Cause 3: eks.mbes-addon-versions-filter-by-cluster-version-is-a-subset — [mismatch] AssertionError: "cert-manager and amazon-sagemaker-hyperpod-training-operator name identical cluster-version sets: ['1.30', '1.31', '1.32', '1.33', '1.34', '1.35', '1.36', '1.37']" (the per-add-on kubernetesVersion filter assertions before it passed, so the filter honours the compatibilities it is given — the compatibilities themselves are identical).
Cause 4: eks.mbes-addon-configuration-names-the-operator-pod-identity-policy — [mismatch] AssertionError: "amazon-sagemaker-hyperpod-training-operator's configurationSchema must be draft-07 JSON Schema; got 'http://json-schema.org/draft-06/schema#'" (the addonName/addonVersion echo in the response matched).

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DescribeAddonVersions (GET /addons/supported-versions, output addons: Addons, httpQuery filters addonName/types/publishers/owners/kubernetesVersion, paginated on nextToken/addons), AddonInfo (addonName, type, addonVersions, publisher, owner, marketplaceInformation, defaultNamespace), AddonVersionInfo (addonVersion, architecture, computeTypes, compatibilities, requiresConfiguration, requiresIamPermissions), Compatibility (clusterVersion, platformVersions, defaultVersion), DescribeAddonConfiguration (GET /addons/configuration-schemas, addonName+addonVersion required httpQuery, output configurationSchema/podIdentityConfiguration).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, point a boto3 eks client at it.
2. Cause 1: describe_addon_versions(addonName='cert-manager') and describe_addon_versions(addonName='amazon-sagemaker-hyperpod-training-operator') -> compare addons[0].owner; both report 'aws'. Then describe_addon_versions(owners=['aws']) and observe that the filter does not separate the two.
3. Cause 2: describe_addon_versions(addonName='aws-efs-csi-driver') -> collect every addonVersions[*].addonVersion; every entry's major component is 2, so the published majors 1 and 3 are unreachable.
4. Cause 3: for each of cert-manager and amazon-sagemaker-hyperpod-training-operator collect the union of addonVersions[*].compatibilities[*].clusterVersion; the two sets are identical (1.30..1.37), so no cluster version exists that empties one add-on's kubernetesVersion-filtered read while matching the other's.
5. Cause 4: describe_addon_configuration(addonName='amazon-sagemaker-hyperpod-training-operator', addonVersion=<its newest listed version>) -> json.loads(configurationSchema)['$schema'] is 'http://json-schema.org/draft-06/schema#' instead of the add-on's own 'http://json-schema.org/draft-07/schema#'.
6. Re-replay the four checks listed in trace_ids to confirm.
