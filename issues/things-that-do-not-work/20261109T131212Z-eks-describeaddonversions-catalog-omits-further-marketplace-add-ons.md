Issue-Source: trace-sweep-divergence
Issue-Trace: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator
Issue-Step: task#35
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator eks-catalog-describe-addon-versions-eks-node-monitoring-agent-vpc-cni
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].marketplaceInformation.productUrl
Issue-Related: 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# eks DescribeAddonVersions catalog omits further Marketplace add-ons and serves a wrong marketplaceInformation.productUrl

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
ONE net-new root cause in the EKS add-on catalog projected by DescribeAddonVersions. It must be closed before this issue is resubmitted.

Cause 1 of 1: a name-filtered DescribeAddonVersions read for a partner/Marketplace add-on (the recording's example is the Datadog operator add-on, addonName 'datadog_operator', type 'monitoring') answers 200 with an empty addons list, and where a Marketplace entry IS served its marketplaceInformation.productUrl does not match the product URL AWS publishes.

AWS: Smithy: DescribeAddonVersions GET /addons/supported-versions -> DescribeAddonVersionsResponse.addons (list of AddonInfo), paginated (inputToken nextToken / items addons / pageSize maxResults), filterable by the httpQuery members addonName, types, publishers, owners and kubernetesVersion; modeled errors InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500). AddonInfo carries addonName, type, addonVersions, publisher, owner, marketplaceInformation and defaultNamespace; MarketplaceInformation carries productId and productUrl ("The product URL from the Amazon Web Services Marketplace"); each AddonVersionInfo carries addonVersion, architecture, computeTypes, compatibilities (one entry per supported Kubernetes minor, each with clusterVersion, platformVersions and defaultVersion), requiresConfiguration and requiresIamPermissions (both @default false). The rule: a read filtered to an add-on name that exists in AWS's catalog returns exactly the one AddonInfo for that name, fully populated — every supported version with its architectures, compute types and per-minor compatibilities, and for a Marketplace-owned add-on the owner/publisher plus marketplaceInformation.productId and productUrl of that listing. Recorded behavior for the specific entries: the catalog contains the monitoring add-on 'datadog_operator' with an addonVersions list whose newest element is the default across the recent minors and whose older elements are non-default, and the Marketplace entries' productUrl is the listing URL of that product.

Emulator: answers 200 with the correct envelope but no entry at $.addons[0] for those add-on names (the filtered read returns an empty list), and for the Marketplace entries it does serve, $.addons[0].marketplaceInformation.productUrl diverges from the recorded product URL. The status, error code and headers are not at issue here — only the reference data in the catalog.

Why: a client that discovers, version-pins or installs a partner/Marketplace add-on sees it as nonexistent, so every downstream CreateAddon/UpdateAddon choice keyed to the catalog (addonVersion, computeTypes, compatibilities, requiresConfiguration, requiresIamPermissions) cannot be made at all; and a subscriber that follows marketplaceInformation.productUrl to accept the seller EULA is sent to the wrong product page, so the subscription step that gates the install can never be completed correctly.

Fix: the static add-on catalog seed behind DescribeAddonVersions in the eks service crate — the catalog data itself (the missing add-on entries and their marketplaceInformation) and the addonName/owners/publishers filter paths that project it. Not the serializer: the response envelope and status are already correct.

Related open issues that do NOT cover this cause: '20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md' (its cause 1 owns the AccuKnox KubeArmor / EG Innovations universal-agent-operator cluster, replay key eks|DescribeAddonVersions|2xx->2xx|$.addons[*]) and '20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md' (wrong version records for add-ons already in the catalog). This replay reports the cluster below as owned by neither; it is a distinct replay unit and a distinct set of catalog entries.

Traces: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator#task#35 (representative; cluster also covers #task#37 of the same fixture and eks-catalog-describe-addon-versions-eks-node-monitoring-agent-vpc-cni#task#17).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (55 fixtures, 393 steps, 249 passed, 144 mismatched, score 63.4%).

Divergence cluster key: service=eks|DescribeAddonVersions|status=200->200|keys=$.addons[0],$.addons[0].marketplaceInformation.productUrl — 3 affected steps, owned by no open issue.

Representative step: DescribeAddonVersions, fixture eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator, step task#35.
  Expected: 200 with $.addons[0] = {'addonName': 'datadog_operator', 'type': 'monitoring', 'addonVersions': [{'addonVersion': 'v0.1.31-eksbuild.1', 'architecture': ['amd64','arm64'], 'computeTypes': ['ec2'], 'compatibilities': [one entry per minor 1.36 down to 1.27, each {'platformVersions': ['*'], 'defaultVersion': True}], 'requiresConfiguration': True, 'requiresIamPermissions': False}, {'addonVersion': 'v0.1.30-eksbuild.1', ... 'defaultVersion': False per minor}, ...]} — i.e. the one populated AddonInfo for the filtered name.
  Actual: 200 with $.addons[0] missing (got None) — the addons list is empty for that name.
  Same cluster also reports $.addons[0].marketplaceInformation.productUrl diverging from the recorded Marketplace product URL.

Other affected steps in the cluster: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator#task#37, eks-catalog-describe-addon-versions-eks-node-monitoring-agent-vpc-cni#task#17.

Note on values: the concrete add-on names, versions, minor lists and product URLs above are the recording's examples of the requirement (the catalog must contain the add-ons AWS publishes, with each version's architectures/computeTypes/compatibilities and the listing's real productId/productUrl); they are not a literal fixture to hard-code per run.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Issue GET /addons/supported-versions?addonName=datadog_operator (fixture eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator, step task#35) and inspect $.addons: the emulator answers 200 with an empty list where AWS returns exactly one populated AddonInfo for that name.
3. Repeat for the other steps in the cluster — the same fixture's task#37 and eks-catalog-describe-addon-versions-eks-node-monitoring-agent-vpc-cni#task#17 — and compare $.addons[0] and $.addons[0].marketplaceInformation.productUrl with the recorded responses.
4. For a Marketplace-owned add-on the catalog DOES serve, compare $.addons[0].marketplaceInformation.productUrl against the recorded product URL for that listing.
