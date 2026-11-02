Issue-Source: trace-sweep-divergence
Issue-Trace: eks-mbes-traj-addon-name-discovery
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-mbes-traj-addon-name-discovery eks-mbes-traj-create-addon-request-mistakes eks-efsfips-traj-addon-name-and-version-mistakes eks-efsfips-traj-catalog-and-schema-discovery eks-ekscc-traj-version-and-support-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].addonVersions[*]
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].owner
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].publisher
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].type
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.eventTtl.constraints.max
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.eventTtl.constraints.min
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.eventTtl.defaultValue
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.serviceNodePortRange.constraints.maxPort.max
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.serviceNodePortRange.constraints.maxPort.min
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.serviceNodePortRange.constraints.minPort.max
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeApiServerConfig.serviceNodePortRange.constraints.minPort.min
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeControllerManagerConfig.horizontalPodAutoscalerControllerConfig.horizontalPodAutoscalerSyncPeriod.constraints.max
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeControllerManagerConfig.podGcControllerConfig.terminatedPodGcThreshold.constraints.max
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeControllerManagerConfig.podGcControllerConfig.terminatedPodGcThreshold.constraints.min
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneComponentConfig.kubeSchedulerConfig.nodeResourcesFit.scoringStrategy.constraints.resources.name.allowedValues[*]
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].controlPlaneScalingTiers[*]
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].endOfExtendedSupportDate
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].endOfStandardSupportDate
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*].releaseDate
Issue-Key: eks|DescribeAddonConfiguration|2xx->4xx|$.addonName
Issue-Key: eks|DescribeAddonConfiguration|2xx->4xx|$.addonVersion
Issue-Key: eks|DescribeAddonConfiguration|2xx->4xx|$.configurationSchema
Issue-Key: eks|DescribeAddonConfiguration|2xx->4xx|$.podIdentityConfiguration
Issue-Key: eks|DescribeAddonConfiguration|2xx->4xx|status
Issue-Key: eks|DescribeAddonConfiguration|4xx->5xx|$.message
Issue-Key: eks|DescribeAddonConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: eks|DescribeAddonConfiguration|4xx->5xx|status
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EKS add-on and cluster-version catalogs serve invented data, and DescribeAddonConfiguration mis-routes

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
Seven net-new root causes share one fault: the read-only EKS catalogs (DescribeAddonVersions, DescribeClusterVersions, DescribeAddonConfiguration) are backed by synthesised data rather than the AWS catalog, and DescribeAddonConfiguration's route does not match the modeled operation when a required query parameter is absent.

The already-open issue 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md owns the clusters where the catalog is simply EMPTY (missing addons[0], missing clusterVersions[0], missing accessPolicies entries, the aws-efs-csi-driver configurationSchema). It does not carry these replay keys, which are a different failure mode: entries that ARE served but carry wrong version numbers, wrong owners/publishers/types, wrong dates and wrong constraint bounds, plus the 404/501 routing defects below.

Cause 1 of 7: DescribeAddonVersions serves invented version entries and the wrong owner.
  AWS: DescribeAddonVersions is GET /addons/supported-versions returning `addons` as a list of AddonInfo (Smithy: DescribeAddonVersions -> DescribeAddonVersionsResponse.addons : Addons; shape AddonInfo members addonName, type, addonVersions, publisher, owner, marketplaceInformation, defaultNamespace). Each AddonVersionInfo carries addonVersion, architecture, computeTypes, compatibilities (clusterVersion / platformVersions / defaultVersion) and the requiresConfiguration / requiresIamPermissions flags. The values are the AWS add-on catalog: for a community add-on, owner 'community', the real published version list and the full compatible-cluster-version set, each entry carrying its computeTypes.
  Emulator: serves a single fabricated version whose addonVersion is not a published one, whose compatibilities list only the newest two cluster versions, which omits computeTypes entirely, and which reports owner 'aws' for a community-owned add-on.
  Why: a client enumerating add-on versions to pick an installable version picks one that does not exist, and a client filtering by owner or by compute type (DescribeAddonVersions has owners and types query filters) gets the wrong set or an empty one.
  Fix: the EKS add-on catalog data behind DescribeAddonVersions' handler.
  Traces: eks-mbes-traj-addon-name-discovery#task#1 (also #task#7, #task#8).

Cause 2 of 7: the AddonInfo type and publisher are invented too.
  AWS: AddonInfo.type and AddonInfo.publisher are catalog values (Smithy: AddonInfo.type : String, AddonInfo.publisher : String) — for the recorded SageMaker add-on, type 'ml-operator' and publisher 'sagemaker'.
  Emulator: type 'machine-learning' and publisher 'eks', together with a fabricated addonVersions entry (wrong version, two compatibilities instead of nine, no computeTypes, requiresIamPermissions inverted).
  Why: the type and publisher taxonomies are what a client filters the catalog on; invented tokens mean the client's filter matches nothing on AWS or matches the wrong add-ons here.
  Fix: the EKS add-on catalog data behind DescribeAddonVersions' handler.
  Traces: eks-mbes-traj-addon-name-discovery#task#2.

Cause 3 of 7: the per-add-on version history is collapsed to one fabricated entry.
  AWS: DescribeAddonVersions returns the add-on's full published version list, newest first, each with its own compatibilities set.
  Emulator: the expected versions are absent altogether; a single unrelated version is served in their place.
  Why: a client pinning or downgrading to a specific add-on version cannot find it in the catalog and treats it as unavailable.
  Fix: the EKS add-on catalog data behind DescribeAddonVersions' handler.
  Traces: eks-mbes-traj-create-addon-request-mistakes#task#7.

Cause 4 of 7: the add-on version served for a named add-on does not match the catalog.
  AWS: for the recorded add-on the newest entry carries its published addonVersion, computeTypes ['ec2','auto'] and defaultVersion true for the compatible cluster version.
  Emulator: a lower, unpublished addonVersion with no computeTypes and defaultVersion false.
  Why: defaultVersion is what a client uses to choose a version when it does not pin one; with no entry flagged default the client has no safe choice, and a missing computeTypes hides whether the add-on runs on the cluster's compute mode.
  Fix: the EKS add-on catalog data behind DescribeAddonVersions' handler.
  Traces: eks-efsfips-traj-addon-name-and-version-mistakes#task#9 (also eks-efsfips-traj-catalog-and-schema-discovery#task#7).

Cause 5 of 7: DescribeClusterVersions serves wrong support dates and wrong control-plane scaling constraints.
  AWS: DescribeClusterVersions is GET /cluster-versions returning ClusterVersionList (Smithy: DescribeClusterVersions -> DescribeClusterVersionsResponse.clusterVersions : ClusterVersionList). Each entry carries releaseDate, endOfStandardSupportDate and endOfExtendedSupportDate as the published EKS lifecycle dates, and controlPlaneScalingTiers whose apiRequestConcurrency and controlPlaneComponentConfigOverrides bounds are the published per-tier limits — for the recorded tier-xl, apiRequestConcurrency 1700 with kubeApiServerConfig and kubeSchedulerConfig overrides null and the kubeControllerManagerConfig terminatedPodGcThreshold constrained to min 10000 / max 12500.
  Emulator: dates offset by a day from the published ones, apiRequestConcurrency 2000 instead of 1700, non-null kubeApiServerConfig/kubeSchedulerConfig overrides the tier does not expose, and terminatedPodGcThreshold constrained to min 1 / max 1000000. It also advertises extra allowed values in the kube-scheduler nodeResourcesFit resources name set.
  Why: a client that computes an upgrade deadline from endOfStandardSupportDate is off by a day, and a client that validates its control-plane configuration against the advertised constraints accepts values AWS will reject (a terminatedPodGcThreshold of 1, or a resource name outside cpu/memory).
  Fix: the EKS cluster-version catalog data behind DescribeClusterVersions' handler.
  Traces: eks-ekscc-traj-version-and-support-mistakes#task#8.

Cause 6 of 7: DescribeAddonConfiguration 404s for a version that has a published schema.
  AWS: DescribeAddonConfiguration is GET /addons/configuration-schemas with required addonName and addonVersion query parameters, returning 200 with addonName, addonVersion, configurationSchema and podIdentityConfiguration (Smithy: DescribeAddonConfiguration, errors InvalidParameterException 400 / ResourceNotFoundException 404).
  Emulator: HTTP 404 with addonName, addonVersion, configurationSchema and podIdentityConfiguration all absent, for an add-on version whose schema the recording returns.
  Why: a client that fetches the configuration schema before building its configurationValues document cannot do so, and concludes the add-on version does not exist.
  Fix: the add-on configuration-schema catalog data behind DescribeAddonConfiguration's handler.
  Traces: eks-efsfips-traj-catalog-and-schema-discovery#task#2.

Cause 7 of 7: DescribeAddonConfiguration without addonVersion returns a 501 NotImplemented instead of the modeled 400.
  AWS: addonVersion is @required on DescribeAddonConfiguration (Smithy: DescribeAddonConfiguration.addonVersion -> String, httpQuery 'addonVersion', required). Omitting it is refused with HTTP 400 and the message 'The addon version parameter is required.' — entirely fixed text — carried by the modeled InvalidParameterException.
  Emulator: HTTP 501 with x-amzn-errortype 'NotImplemented' and the message 'operation GET /addons/configuration-schemas is not implemented in this emulator'. The route only matches when both query parameters are present, so a missing required parameter falls through to the unimplemented-operation fallback even though the operation IS implemented (the same route answers when addonVersion is supplied).
  Why: 501 is a server-fault class a client retries or escalates, and 'not implemented in this emulator' tells the client the operation is unsupported rather than that its request is missing a parameter. A client probing which operations exist gets the wrong answer about this one.
  Fix: DescribeAddonConfiguration's routing and validation — the route must match on path and method and then enforce the required query parameters, raising InvalidParameterException (400) rather than deferring to the not-implemented fallback.
  Traces: eks-efsfips-traj-catalog-and-schema-discovery#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — eks-mbes-traj-addon-name-discovery#task#1, DescribeAddonVersions, 200->200: $.addons[0].addonVersions[0] expected {'addonVersion': 'v1.21.2-eksbuild.4', 'architecture': ['amd64','arm64'], 'computeTypes': ['auto','ec2','fargate','hybrid'], 'compatibilities': six entries 1.37..1.32 each defaultVersion True, requiresConfiguration False, requiresIamPermissions False}; got {'addonVersion': 'v1.23.2-eksbuild.1', 'architecture': ['amd64','arm64'], 'compatibilities': [1.36 defaultVersion False, 1.37 defaultVersion True], ...} with no computeTypes. $.addons[0].owner expected 'community', got 'aws'. 3 affected steps.
Cause 2 — eks-mbes-traj-addon-name-discovery#task#2: $.addons[0].type expected 'ml-operator', got 'machine-learning'; $.addons[0].publisher expected 'sagemaker', got 'eks'; $.addons[0].addonVersions[0] expected v1.3.0-eksbuild.1 with computeTypes ['hyperpod'], nine compatibilities (1.36..1.28, all defaultVersion True) and requiresIamPermissions True; got v1.7.0-eksbuild.1, no computeTypes, two compatibilities, requiresIamPermissions False.
Cause 3 — eks-mbes-traj-create-addon-request-mistakes#task#7: $.addons[0].addonVersions[0] (expected v1.5.9-eksbuild.1 with computeTypes ['ec2'] and eleven compatibilities 1.33..1.23) and $.addons[0].addonVersions[1] (expected v1.5.8-eksbuild.1) both missing; the emulator served v2.7.7-eksbuild.1 with two compatibilities instead.
Cause 4 — eks-efsfips-traj-addon-name-and-version-mistakes#task#9: $.addons[0].addonVersions[0] expected {'addonVersion': 'v3.5.2-eksbuild.1', 'computeTypes': ['ec2','auto'], 'compatibilities': [{'clusterVersion':'1.35','defaultVersion':True}], ...}; got {'addonVersion': 'v2.6.7-eksbuild.1', no computeTypes, 'compatibilities': [{'clusterVersion':'1.35','defaultVersion':False}]}. 2 affected steps.
Cause 5 — eks-ekscc-traj-version-and-support-mistakes#task#8, DescribeClusterVersions: $.clusterVersions[0].releaseDate expected 1748476800.0 got 1748390400; endOfStandardSupportDate expected 1785283200.0 got 1785196800; endOfExtendedSupportDate expected 1816819200.0 got 1816732800; controlPlaneScalingTiers[0] expected apiRequestConcurrency 1700 with kubeApiServerConfig/kubeSchedulerConfig overrides None and terminatedPodGcThreshold constraints {min:10000,max:12500}, got apiRequestConcurrency 2000 with populated kubeApiServerConfig/kubeSchedulerConfig overrides and terminatedPodGcThreshold constraints {min:1,max:1000000}; plus extra entries at controlPlaneComponentConfig.kubeSchedulerConfig.nodeResourcesFit.scoringStrategy.constraints.resources.name.allowedValues[2..4].
Cause 6 — eks-efsfips-traj-catalog-and-schema-discovery#task#2, DescribeAddonConfiguration: expected status 200 with $.addonName 'aws-efs-csi-driver', $.addonVersion 'v1.5.8-eksbuild.1' and $.configurationSchema populated; got status 404 with addonName, addonVersion, configurationSchema and podIdentityConfiguration all missing.
Cause 7 — eks-efsfips-traj-catalog-and-schema-discovery#task#3, DescribeAddonConfiguration: expected status 400 and $.message 'The addon version parameter is required.'; got status 501, $.message 'operation GET /addons/configuration-schemas is not implemented in this emulator', $headers.x-amzn-errortype 'NotImplemented'.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DescribeAddonVersions (GET /addons/supported-versions, 200, output addons: Addons, query filters addonName/types/publishers/owners/kubernetesVersion), shape AddonInfo (addonName, type, addonVersions, publisher, owner, marketplaceInformation, defaultNamespace), DescribeAddonConfiguration (GET /addons/configuration-schemas, addonName AND addonVersion both httpQuery + required, errors InvalidParameterException 400 / ResourceNotFoundException 404 / ServerException 500), DescribeClusterVersions (GET /cluster-versions, output clusterVersions: ClusterVersionList).

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-mbes-traj-addon-name-discovery, step task#1; the other six at the fixture#step pairs listed per cause.
4. Manually: GET /addons/supported-versions?addonName=<an add-on> and compare addons[0].owner / .type / .publisher and the addonVersions list (addonVersion, computeTypes, compatibilities, defaultVersion) against the AWS catalog — the emulator serves one synthesised entry with no computeTypes and no default-flagged version. GET /cluster-versions and compare releaseDate / endOfStandardSupportDate / endOfExtendedSupportDate and controlPlaneScalingTiers[0].apiRequestConcurrency and its constraint bounds. GET /addons/configuration-schemas?addonName=aws-efs-csi-driver&addonVersion=v1.5.8-eksbuild.1 -> 404 instead of the schema. GET /addons/configuration-schemas?addonName=aws-efs-csi-driver (no addonVersion) -> 501 NotImplemented instead of 400 'The addon version parameter is required.'
