// SPDX-License-Identifier: CC0-1.0

//! The add-on catalog: the builds, compatibilities and configuration schemas this
//! service publishes for the Amazon EKS add-ons, the records they are read back
//! as, and the seed that puts them in the store.

use super::*;

/// One row of the add-on catalog seed: an Amazon EKS-managed add-on, and the
/// version line its published builds run along.
///
/// The catalog is a PUBLICATION, like the Kubernetes version calendar
/// ([`VERSION_CALENDAR`]): it is what Amazon EKS offers, not per-account data,
/// so it is seeded once and no operation writes it.
pub(super) struct AddonCatalogSeed {
    /// The add-on's name, the value `CreateAddon$addonName` takes.
    pub(super) addon_name: &'static str,
    /// Who owns the add-on ([`ADDON_OWNER_AWS`] or
    /// [`ADDON_OWNER_MARKETPLACE`]), which `DescribeAddonVersions$owners`
    /// filters on.
    pub(super) owner: &'static str,
    /// Who publishes the add-on ([`ADDON_PUBLISHER_EKS`] for an Amazon
    /// EKS-managed add-on, the vendor for a Marketplace one), which
    /// `DescribeAddonVersions$publishers` filters on.
    pub(super) publisher: &'static str,
    /// The category Amazon EKS publishes the add-on under
    /// (`AddonInfo$type`), which `DescribeAddonVersions$types` filters on.
    pub(super) addon_type: &'static str,
    /// The Kubernetes namespace the add-on's workloads are installed into
    /// (`AddonInfo$defaultNamespace`).
    pub(super) default_namespace: &'static str,
    /// How the add-on's pods are placed, which decides what its install needs
    /// from the cluster: a [`AddonWorkload::PerNode`] add-on has nothing to
    /// schedule on a cluster with no nodes, a [`AddonWorkload::Replicated`] one
    /// must get a replica scheduled somewhere.
    ///
    /// Source: the workloads each add-on's chart ships — the node agents and
    /// device plugins are DaemonSets, the controllers, operators and servers are
    /// Deployments.
    pub(super) workload: AddonWorkload,
    /// The add-ons whose Kubernetes resources this add-on's own resources are
    /// applied against: the ones its documented install requires on the cluster
    /// FIRST, because its custom resources, certificates or webhooks come from
    /// them. EMPTY for an add-on that needs nothing but the cluster.
    ///
    /// Source: the prerequisites the Amazon EKS User Guide lists for the add-on.
    pub(super) required_addons: &'static [&'static str],
    /// Whether the add-on serves an admission webhook the API server calls
    /// before it admits the resources of an add-on installed against this one.
    ///
    /// Source: the webhook key the add-on's own configuration schema declares
    /// ([`AddonCatalogSeed::configuration_properties`]).
    pub(super) admission_webhook: bool,
    /// Whether the add-on needs IAM permissions of its own
    /// (`AddonVersionInfo$requiresIamPermissions`).
    pub(super) requires_iam_permissions: bool,
    /// The configuration properties the add-on cannot be installed without.
    /// EMPTY for an add-on whose configuration values are optional, which is
    /// what makes `AddonVersionInfo$requiresConfiguration` false for it and
    /// what leaves the published configuration schema with no required
    /// property: the flag and the schema have this one source.
    pub(super) required_configuration: &'static [&'static str],
    /// The top-level keys this add-on's OWN configuration schema declares, each
    /// with the JSON type that key takes — published beside the keys every
    /// add-on's schema carries ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), and
    /// beside the ones the install itself takes (see
    /// [`AddonCatalogRecord::configuration_schema`]). A key named here with a
    /// type of its own OVERRIDES the common one.
    ///
    /// Source: the schema each add-on publishes, which is the set of values its
    /// own chart exposes — `aws eks describe-addon-configuration --addon-name
    /// <name> --addon-version <build>` reports it, and the Amazon EKS User
    /// Guide's configuration examples for an add-on set keys from this list
    /// (`aws-efs-csi-driver`'s `useFIPS` is the one the guide's FIPS install
    /// passes). Keys are written in the schema's own alphabetical order.
    pub(super) configuration_properties: &'static [AddonSchemaProperty],
    /// The Kubernetes service accounts the add-on runs as, each with the
    /// managed policies Amazon EKS recommends for it — what
    /// `DescribeAddonConfiguration$podIdentityConfiguration` reports ("The
    /// Kubernetes service account name used by the add-on, and any suggested
    /// IAM policies"). Empty for an add-on that runs as no service account of
    /// its own.
    pub(super) pod_identity: &'static [(&'static str, &'static [&'static str])],
    /// `(major, minor, patch)` of the build published for the OLDEST Kubernetes
    /// version in [`VERSION_CALENDAR`]. An add-on's version tracks the ADD-ON,
    /// not the control plane, so each add-on carries its own line.
    pub(super) oldest_build: (u32, u32, u32),
}

/// The add-ons this service publishes.
///
/// The Amazon EKS-managed rows are the add-ons the "Amazon EKS add-ons" section
/// of the Amazon EKS User Guide publishes — the ones Amazon EKS builds itself
/// and the community projects it curates alongside them — with the name a caller
/// installs each by, the category it is published under, the namespace its
/// workloads run in, whether it needs IAM permissions of its own, EVERY service
/// account it runs as — a driver whose controller and node components both call
/// AWS carries one per component — with the managed policies the guide
/// recommends for each, and the keys its own configuration schema declares. They
/// are owned by [`ADDON_OWNER_AWS`] and published by [`ADDON_PUBLISHER_EKS`].
///
/// This table is what the catalog PUBLISHES, so an add-on missing from it cannot
/// be discovered or installed at all: a row here is the only way a caller reads
/// an add-on's builds, its compatibilities and its configuration surface.
///
/// The rest are AWS Marketplace add-ons ([`ADDON_OWNER_MARKETPLACE`]), published
/// by their VENDOR and carrying the `marketplaceInformation` of the listing they
/// are subscribed through. They are in the catalog because the owner, publisher
/// and `requiresConfiguration` an add-on reports are properties of the add-on,
/// not of Amazon EKS: a Marketplace add-on that cannot run without its vendor's
/// credentials is the kind that reports `requiresConfiguration` true, and the
/// configuration properties named here are the ones that vendor's own
/// documentation requires.
///
/// Rows are written in add-on NAME order, which is the order the catalog is
/// published in ([`seed_addon_catalog`]). The table is fixed, so it also bounds
/// this part of the store.
pub(super) const ADDON_CATALOG: &[AddonCatalogSeed] = &[
    AddonCatalogSeed {
        addon_name: "adot",
        // The OpenTelemetry Operator, whose admission webhooks are served with
        // certificates cert-manager issues.
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "opentelemetry-operator-system",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("affinity", "object"),
            ("collector", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (0, 102, 1),
    },
    AddonCatalogSeed {
        addon_name: "amazon-cloudwatch-observability",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "amazon-cloudwatch",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "cloudwatch-agent",
            &[
                "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
                "arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess",
            ],
        )],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("agent", "object"),
            ("containerLogs", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 5, 0),
    },
    AddonCatalogSeed {
        // Amazon SageMaker HyperPod's task governance, which schedules the
        // training and inference workloads of a HyperPod cluster whose
        // orchestration runs on this cluster.
        addon_name: "amazon-sagemaker-hyperpod-taskgovernance",
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // Its chart exposes the queue the governance controller schedules
        // through and how many replicas of that controller run, beside the
        // placement values every add-on's schema carries.
        configuration_properties: &[
            ("kueue", "object"),
            ("replicaCount", "integer"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "amazon-sagemaker-hyperpod-training-operator",
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: "aws-hyperpod",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("hpTrainingControllerManager", "object"),
            ("nodeSelector", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "aws-ebs-csi-driver",
        // The CSI drivers run a controller Deployment beside their node
        // DaemonSet, so an install of one has a replica to place.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "ebs-csi-controller-sa",
            &["arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"],
        )],
        configuration_properties: &[
            ("additionalDaemonSets", "object"),
            ("controller", "object"),
            ("defaultStorageClass", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
        ],
        oldest_build: (1, 31, 0),
    },
    AddonCatalogSeed {
        // The Elastic Fabric Adapter device plugin, which advertises the EFA
        // network interfaces of a node to the scheduler.
        addon_name: "aws-efa-k8s-device-plugin",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // The plugin is a DaemonSet that advertises a node's devices and takes
        // no settings of its own beyond the placement values every add-on's
        // schema carries.
        configuration_properties: &[],
        oldest_build: (0, 5, 3),
    },
    AddonCatalogSeed {
        addon_name: "aws-efs-csi-driver",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        // BOTH of the driver's components call Amazon EFS: the controller
        // Deployment provisions access points and the node DaemonSet resolves the
        // mount targets it mounts through, so the catalog carries an entry per
        // service account the driver runs as — not just the controller's — each
        // recommending the policy Amazon EKS publishes for the driver.
        pod_identity: &[
            (
                "efs-csi-controller-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
            (
                "efs-csi-node-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
        ],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
            // The key the guide's FIPS install sets: an add-on's schema
            // declares the options that add-on itself takes.
            ("useFIPS", "boolean"),
        ],
        oldest_build: (2, 0, 7),
    },
    AddonCatalogSeed {
        addon_name: "aws-guardduty-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "amazon-guardduty",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("priorityClassName", "string"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 6, 1),
    },
    AddonCatalogSeed {
        addon_name: "aws-mountpoint-s3-csi-driver",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 7, 0),
    },
    AddonCatalogSeed {
        // The agent that reports the cluster's network flows to Amazon
        // CloudWatch Network Flow Monitor, which it publishes to with
        // permissions of its own.
        addon_name: "aws-network-flow-monitoring-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "aws-network-flow-monitor",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "aws-network-flow-monitor-agent",
            &["arn:aws:iam::aws:policy/CloudWatchNetworkFlowMonitorAgentPublishPolicy"],
        )],
        // Its chart exposes the agent itself; the rest of what a cluster
        // operator sets on it are the placement values every add-on's schema
        // carries.
        configuration_properties: &[("agent", "object")],
        oldest_build: (1, 0, 1),
    },
    AddonCatalogSeed {
        addon_name: "cert-manager",
        // Its controller, cainjector and webhook are Deployments, and the
        // webhook is the one the API server calls for the resources of the
        // add-ons installed against it.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "cert-manager",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("cainjector", "object"),
            ("installCRDs", "boolean"),
            ("prometheus", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("webhook", "object"),
        ],
        oldest_build: (1, 16, 2),
    },
    AddonCatalogSeed {
        addon_name: "coredns",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("autoScaling", "object"),
            ("computeType", "string"),
            ("corefile", "string"),
            ("replicaCount", "integer"),
        ],
        oldest_build: (1, 11, 1),
    },
    AddonCatalogSeed {
        addon_name: "datadog_operator",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "datadog",
        addon_type: "monitoring",
        default_namespace: "datadog-agent",
        requires_iam_permissions: false,
        // The Datadog Operator sends telemetry to a Datadog organisation, which
        // it cannot do without that organisation's API key.
        required_configuration: &["apiKey"],
        pod_identity: &[],
        configuration_properties: &[
            ("clusterName", "string"),
            ("resources", "object"),
            ("site", "string"),
        ],
        oldest_build: (1, 8, 0),
    },
    AddonCatalogSeed {
        addon_name: "dynatrace_dynatrace-operator",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "dynatrace",
        addon_type: "monitoring",
        default_namespace: "dynatrace",
        requires_iam_permissions: false,
        // The Dynatrace Operator connects to one Dynatrace environment, which it
        // cannot do without that environment's API URL and token.
        required_configuration: &["apiUrl", "apiToken"],
        pod_identity: &[],
        configuration_properties: &[("resources", "object"), ("tolerations", "array")],
        oldest_build: (1, 3, 0),
    },
    AddonCatalogSeed {
        addon_name: "eks-node-monitoring-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "eks-pod-identity-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("agent", "object"),
            ("nodeSelector", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 2, 0),
    },
    AddonCatalogSeed {
        addon_name: "external-dns",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: "external-dns",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "external-dns",
            &["arn:aws:iam::aws:policy/AmazonRoute53FullAccess"],
        )],
        configuration_properties: &[
            ("domainFilters", "array"),
            ("policy", "string"),
            ("provider", "string"),
            ("resources", "object"),
            ("sources", "array"),
            ("txtOwnerId", "string"),
        ],
        oldest_build: (0, 15, 0),
    },
    AddonCatalogSeed {
        addon_name: "kube-proxy",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // kube-proxy is the one add-on whose version IS the Kubernetes version
        // it runs with, so its line starts at the oldest calendar row's version.
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 30, 0),
    },
    AddonCatalogSeed {
        addon_name: "kube-state-metrics",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("collectors", "array"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (2, 13, 0),
    },
    AddonCatalogSeed {
        addon_name: "kubecost_kubecost",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "kubecost",
        addon_type: "cost-management",
        default_namespace: "kubecost",
        requires_iam_permissions: false,
        // Kubecost reports on the cluster it runs in, so it needs no vendor
        // credentials: a Marketplace add-on requires configuration only when it
        // cannot run without it.
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("global", "object"),
            ("kubecostToken", "string"),
            ("persistentVolume", "object"),
            ("prometheus", "object"),
        ],
        oldest_build: (2, 3, 0),
    },
    AddonCatalogSeed {
        addon_name: "metrics-server",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("args", "array"),
            ("nodeSelector", "object"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (0, 7, 0),
    },
    AddonCatalogSeed {
        addon_name: "prometheus-node-exporter",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "prometheus-node-exporter",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("hostNetwork", "boolean"),
            ("resources", "object"),
            ("service", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 8, 2),
    },
    AddonCatalogSeed {
        addon_name: "snapshot-controller",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("volumeSnapshotClasses", "array"),
        ],
        oldest_build: (8, 1, 0),
    },
    AddonCatalogSeed {
        addon_name: "vpc-cni",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[("aws-node", &["arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"])],
        configuration_properties: &[
            ("branchENICooldown", "integer"),
            // `enableNetworkPolicy` and `enableWindowsIpam` are STRINGS in this
            // add-on's schema, not booleans: the key a client sets is the one
            // the schema declares, with the type the schema gives it.
            ("enableNetworkPolicy", "string"),
            ("enableWindowsIpam", "string"),
            ("eniConfig", "object"),
            ("env", "object"),
            ("init", "object"),
            ("nodeAgent", "object"),
        ],
        oldest_build: (1, 18, 0),
    },
];

/// One Kubernetes version a published add-on build supports, as the catalog
/// holds it: the modelled `Compatibility`.
#[derive(Debug, Clone)]
pub struct AddonCompatibilityRecord {
    /// The Kubernetes minor version this build supports, a
    /// [`VERSION_CALENDAR`] row.
    pub cluster_version: String,
    /// The platform versions of that cluster version the build runs on
    /// ([`ADDON_ANY_PLATFORM_VERSION`]).
    pub platform_versions: Vec<String>,
    /// Whether this build is the one `CreateAddon` installs on a cluster of
    /// that Kubernetes version when the caller names no `addonVersion`.
    pub default_version: bool,
}

impl AddonCompatibilityRecord {
    /// This row as the modelled `Compatibility` shape.
    pub fn to_compatibility(&self) -> Compatibility {
        Compatibility {
            cluster_version: Some(self.cluster_version.clone()),
            platform_versions: Some(self.platform_versions.clone()),
            default_version: self.default_version,
        }
    }
}

/// One published build of one add-on, as the catalog holds it: the modelled
/// `AddonVersionInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogVersionRecord {
    /// The build's version, in the `vX.Y.Z-eksbuild.N` form every Amazon EKS
    /// add-on version takes.
    pub addon_version: String,
    /// When Amazon EKS published this build — the release date of the
    /// Kubernetes version it was published for. A build the clock has not
    /// reached is not one the catalog offers yet, exactly as an unreleased
    /// Kubernetes version is not ([`ClusterVersionRecord::is_released`]).
    pub published_at: SystemTime,
    /// The architectures the build is published for ([`ADDON_ARCHITECTURES`]).
    pub architecture: Vec<String>,
    /// The Kubernetes versions the build supports, oldest first.
    pub compatibilities: Vec<AddonCompatibilityRecord>,
    /// The configuration properties the build cannot be installed without; see
    /// [`AddonCatalogSeed::required_configuration`].
    pub required_configuration: Vec<String>,
    /// Whether the build needs IAM permissions of its own.
    pub requires_iam_permissions: bool,
}

impl AddonCatalogVersionRecord {
    /// Whether Amazon EKS has published this build at `now`.
    pub fn is_published(&self, now: SystemTime) -> bool {
        now >= self.published_at
    }

    /// Whether this build supports a Kubernetes version.
    pub fn supports(&self, cluster_version: &str) -> bool {
        self.compatibilities
            .iter()
            .any(|entry| entry.cluster_version == cluster_version)
    }

    /// Whether this build cannot be installed without configuration values —
    /// `AddonVersionInfo$requiresConfiguration`, which is exactly "this build
    /// has a required configuration property".
    pub fn requires_configuration(&self) -> bool {
        !self.required_configuration.is_empty()
    }

    /// Whether this build is the default one for a Kubernetes version.
    pub fn is_default_for(&self, cluster_version: &str) -> bool {
        self.compatibilities
            .iter()
            .any(|entry| entry.cluster_version == cluster_version && entry.default_version)
    }

    /// This build as the modelled `AddonVersionInfo` shape.
    ///
    /// `cluster_version`, when given, is the Kubernetes version the listing was
    /// filtered to: the reported compatibilities are then the ones for that
    /// version alone, because a filtered listing answers about that version.
    pub fn to_addon_version_info(&self, cluster_version: Option<&str>) -> AddonVersionInfo {
        AddonVersionInfo {
            addon_version: Some(self.addon_version.clone()),
            architecture: Some(self.architecture.clone()),
            // `computeTypes` is "The type of compute associated with the
            // add-on version": no build this catalog publishes is restricted to
            // one, so there is nothing to report.
            compute_types: None,
            compatibilities: Some(
                self.compatibilities
                    .iter()
                    .filter(|entry| {
                        cluster_version.is_none_or(|wanted| wanted == entry.cluster_version)
                    })
                    .map(AddonCompatibilityRecord::to_compatibility)
                    .collect(),
            ),
            requires_configuration: self.requires_configuration(),
            requires_iam_permissions: self.requires_iam_permissions,
        }
    }
}

/// One add-on the catalog publishes, with every build of it: the modelled
/// `AddonInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogRecord {
    pub addon_name: String,
    pub addon_type: String,
    pub owner: String,
    pub publisher: String,
    pub default_namespace: String,
    /// How this add-on's pods are placed; see [`AddonCatalogSeed::workload`].
    pub workload: AddonWorkload,
    /// The add-ons this one's resources are applied against; see
    /// [`AddonCatalogSeed::required_addons`].
    pub required_addons: Vec<String>,
    /// Whether this add-on serves an admission webhook; see
    /// [`AddonCatalogSeed::admission_webhook`].
    pub admission_webhook: bool,
    /// The AWS Marketplace listing this add-on is subscribed through, for an
    /// add-on owned by [`ADDON_OWNER_MARKETPLACE`]; `None` for an Amazon
    /// EKS-managed one, which is not listed on Marketplace at all.
    pub marketplace_information: Option<MarketplaceInformation>,
    /// The service accounts the add-on runs as, with the managed policies
    /// recommended for each; see [`AddonCatalogSeed::pod_identity`].
    pub pod_identity: Vec<(String, Vec<String>)>,
    /// The keys this add-on's configuration schema declares, each with the JSON
    /// type it takes: its own ([`AddonCatalogSeed::configuration_properties`])
    /// merged with the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), in the schema's own
    /// alphabetical order.
    pub configuration_properties: Vec<(String, String)>,
    /// Every build of this add-on, NEWEST FIRST — the order the catalog
    /// publishes versions in, so a caller reading it to pick a version sees the
    /// newest build at the head of the list.
    pub versions: Vec<AddonCatalogVersionRecord>,
}

impl AddonCatalogRecord {
    /// The builds of this add-on Amazon EKS has published at `now`, newest
    /// first.
    pub fn published_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &AddonCatalogVersionRecord> + '_ {
        self.versions
            .iter()
            .filter(move |version| version.is_published(now))
    }

    /// This add-on as the modelled `AddonInfo` shape, reporting `versions`.
    pub fn to_addon_info(&self, versions: Vec<AddonVersionInfo>) -> AddonInfo {
        AddonInfo {
            addon_name: Some(self.addon_name.clone()),
            r#type: Some(self.addon_type.clone()),
            addon_versions: Some(versions),
            publisher: Some(self.publisher.clone()),
            owner: Some(self.owner.clone()),
            marketplace_information: self.marketplace_information.clone(),
            default_namespace: Some(self.default_namespace.clone()),
        }
    }

    /// The service accounts this add-on runs as, as the modelled
    /// `AddonPodIdentityConfiguration` list.
    pub fn to_pod_identity_configuration(&self) -> Vec<AddonPodIdentityConfiguration> {
        self.pod_identity
            .iter()
            .map(|(service_account, policies)| AddonPodIdentityConfiguration {
                service_account: Some(service_account.clone()),
                recommended_managed_policies: Some(policies.clone()),
            })
            .collect()
    }

    /// The JSON schema `DescribeAddonConfiguration` publishes for one build of
    /// this add-on: "A JSON schema that's used to validate the configuration
    /// values you provide when an add-on is created or updated".
    ///
    /// The properties it declares are THIS add-on's own configurable keys
    /// ([`AddonCatalogSeed::configuration_properties`], each with the JSON type
    /// that key takes) and the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), beside the two the install
    /// itself takes — the namespace the add-on is installed into and the service
    /// account it runs as — and the properties the build cannot be installed
    /// without. A document may set no other key, which is what
    /// `additionalProperties: false` states, so a client reading the schema
    /// learns exactly the configuration surface of this add-on rather than a
    /// document shared by every one of them. Its `required` list is the build's
    /// own
    /// [`AddonCatalogVersionRecord::required_configuration`], which is also what
    /// makes [`AddonCatalogVersionRecord::requires_configuration`] true: a build
    /// that requires configuration publishes a schema that requires it, and one
    /// that does not publishes a schema with nothing required.
    pub fn configuration_schema(&self, version: &AddonCatalogVersionRecord) -> String {
        self.configuration_schema_value(version).to_string()
    }

    /// That same published schema as the JSON document it is, which is what
    /// [`Self::configuration_violation`] holds a configuration document to:
    /// ONE source, with no serialise-and-parse round trip between the schema a
    /// client reads and the schema a create is judged by.
    pub(super) fn configuration_schema_value(
        &self,
        version: &AddonCatalogVersionRecord,
    ) -> ::serde_json::Value {
        let mut properties = ::serde_json::Map::new();
        properties.insert(
            "namespace".to_string(),
            ::serde_json::json!({
                "type": "string",
                "default": self.default_namespace,
                "description": "The Kubernetes namespace the add-on is installed into.",
            }),
        );
        if let Some((service_account, _)) = self.pod_identity.first() {
            properties.insert(
                "serviceAccount".to_string(),
                ::serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "default": service_account,
                            "description":
                                "The Kubernetes service account the add-on runs as.",
                        },
                    },
                }),
            );
        }
        for (property, json_type) in &self.configuration_properties {
            properties.insert(
                property.clone(),
                ::serde_json::json!({ "type": json_type }),
            );
        }
        for property in &version.required_configuration {
            // A required key keeps the type the add-on's own schema gives it —
            // only the "you cannot install without this" description is added —
            // so a required property of a type other than string cannot be
            // mis-declared here and then refused when a caller sets it.
            let declared = properties
                .get(property)
                .and_then(|declared| declared.get("type"))
                .and_then(::serde_json::Value::as_str)
                .unwrap_or("string")
                .to_string();
            properties.insert(
                property.clone(),
                ::serde_json::json!({
                    "type": declared,
                    "description": format!(
                        "Required to install {}: the add-on cannot run without it.",
                        self.addon_name
                    ),
                }),
            );
        }
        ::serde_json::json!({
            "$schema": ADDON_CONFIGURATION_SCHEMA_DIALECT,
            "type": "object",
            "properties": ::serde_json::Value::Object(properties),
            "required": version.required_configuration,
            // A key the schema does not declare is not a key this add-on takes,
            // so a document that sets one does not satisfy the schema.
            "additionalProperties": false,
        })
    }

    /// How `document` fails the configuration schema this entry publishes for
    /// `version` — or `None` when it satisfies that schema.
    ///
    /// "The values that you provide are validated against the schema returned by
    /// DescribeAddonConfiguration" (`CreateAddon$configurationValues`), so this
    /// validates against THAT document — the one
    /// [`Self::configuration_schema_value`] builds and
    /// [`Self::configuration_schema`] publishes — which is what keeps what the
    /// schema declares and what a document is held to from ever disagreeing.
    pub fn configuration_violation(
        &self,
        version: &AddonCatalogVersionRecord,
        document: &::serde_json::Value,
    ) -> Option<String> {
        json_schema_violation(
            JSON_SCHEMA_ROOT_PATH,
            &self.configuration_schema_value(version),
            document,
        )
    }
}

/// The path a JSON Schema validator names the whole document by, which every
/// violation below is reported relative to.
pub(super) const JSON_SCHEMA_ROOT_PATH: &str = "$";

/// How `value` fails `schema`, as a validator states it — the path at fault and
/// what is wrong with it — or `None` when it satisfies the schema.
///
/// It reads the four keywords the add-on configuration schemas above are written
/// with: `type`, `properties`, `required` and `additionalProperties`. The first
/// violation found is the one reported, and the walk is depth-first through the
/// properties the SCHEMA declares, so its depth is bounded by the nesting of
/// these schemas rather than by the document a caller sent; the document itself
/// is parsed by `serde_json`, not here.
pub(super) fn json_schema_violation(
    path: &str,
    schema: &::serde_json::Value,
    value: &::serde_json::Value,
) -> Option<String> {
    if let Some(expected) = schema.get("type").and_then(::serde_json::Value::as_str) {
        if !json_value_has_type(expected, value) {
            return Some(format!(
                "{path}: {} found, {expected} expected",
                json_type_name(value)
            ));
        }
    }
    let members = value.as_object()?;
    let properties = schema
        .get("properties")
        .and_then(::serde_json::Value::as_object);
    // `additionalProperties: false` is what makes a key the schema does not
    // declare a violation rather than a value carried along unread.
    if schema.get("additionalProperties") == Some(&::serde_json::Value::Bool(false)) {
        if let Some(undeclared) = members.keys().find(|key| {
            properties.is_none_or(|declared| !declared.contains_key(key.as_str()))
        }) {
            return Some(format!(
                "{path}.{undeclared}: is not defined in the schema and the schema does not \
                 allow additional properties"
            ));
        }
    }
    for required in schema
        .get("required")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let Some(property) = required
            .as_str()
            .filter(|property| !members.contains_key(*property))
        {
            return Some(format!("{path}.{property}: is missing but it is required"));
        }
    }
    let properties = properties?;
    members.iter().find_map(|(key, member)| {
        let declared = properties.get(key)?;
        json_schema_violation(&format!("{path}.{key}"), declared, member)
    })
}

/// Whether `value` is of the JSON Schema `type` named by `expected`.
///
/// A type the add-on configuration schemas do not use constrains nothing, which
/// is what an unknown keyword value means in JSON Schema.
pub(super) fn json_value_has_type(expected: &str, value: &::serde_json::Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        // In the dialect these schemas name, an integer is any number with a
        // zero fractional part, so `2.0` is an integer and `1.5` is not.
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

/// The JSON Schema type name of a value, as a violation names what it found.
pub(super) fn json_type_name(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "boolean",
        ::serde_json::Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        ::serde_json::Value::String(_) => "string",
        ::serde_json::Value::Array(_) => "array",
        ::serde_json::Value::Object(_) => "object",
    }
}

/// The AWS Marketplace listing one Marketplace add-on is subscribed through.
///
/// `MarketplaceInformation` is the listing's `productId` and `productUrl`. This
/// emulator holds no AWS Marketplace subscription, so there is no real listing
/// to report: both are DERIVED from the add-on's name through the one
/// derivation this service's UUID-shaped identifiers use
/// ([`derive_uuid_from_seed`]), which makes them distinct per add-on and stable
/// for as long as the catalog publishes it — the properties a caller can rely
/// on — and the URL is the `prodview-<id>` form a Marketplace listing is
/// published at.
pub(super) fn marketplace_listing(addon_name: &str) -> MarketplaceInformation {
    let product_id = derive_uuid_from_seed(addon_name);
    let view_id: String = product_id.chars().filter(char::is_ascii_alphanumeric).take(13).collect();
    MarketplaceInformation {
        product_id: Some(product_id),
        product_url: Some(format!(
            "https://aws.amazon.com/marketplace/pp/prodview-{view_id}"
        )),
    }
}

/// Build the add-on catalog this service publishes from [`ADDON_CATALOG`].
///
/// An add-on publishes one build per Kubernetes minor version the calendar
/// carries, along its own version line: the build published alongside a
/// Kubernetes version supports that version and the one before it — an add-on
/// build keeps working on the cluster version it was released on — and it is the
/// DEFAULT version for the Kubernetes version it was published for. So for any
/// Kubernetes version the catalog names exactly ONE default build per add-on,
/// while the newer build that still supports that version is offered as a
/// non-default choice, which is what `CreateAddon` resolves an absent
/// `addonVersion` through ([`ComAmazonawsEksAwswesleyfrontendState::default_addon_version`]).
///
/// Each add-on's builds come out NEWEST FIRST, the order the catalog publishes
/// them in; the add-ons themselves keep [`ADDON_CATALOG`]'s name order.
pub(super) fn seed_addon_catalog() -> Vec<AddonCatalogRecord> {
    ADDON_CATALOG
        .iter()
        .map(|seed| {
            let (major, minor, patch) = seed.oldest_build;
            let mut versions: Vec<AddonCatalogVersionRecord> = VERSION_CALENDAR
                .iter()
                .enumerate()
                .map(|(index, (cluster_version, release, _, _))| {
                    let mut compatibilities = Vec::with_capacity(2);
                    if let Some((previous, _, _, _)) = index.checked_sub(1).and_then(|earlier| VERSION_CALENDAR.get(earlier))
                    {
                        compatibilities.push(AddonCompatibilityRecord {
                            cluster_version: (*previous).to_string(),
                            platform_versions: vec![ADDON_ANY_PLATFORM_VERSION.to_string()],
                            default_version: false,
                        });
                    }
                    compatibilities.push(AddonCompatibilityRecord {
                        cluster_version: (*cluster_version).to_string(),
                        platform_versions: vec![ADDON_ANY_PLATFORM_VERSION.to_string()],
                        default_version: true,
                    });
                    AddonCatalogVersionRecord {
                        addon_version: format!(
                            "v{major}.{}.{patch}-eksbuild.1",
                            minor + index as u32
                        ),
                        published_at: calendar_instant(release),
                        architecture: ADDON_ARCHITECTURES
                            .iter()
                            .map(|architecture| (*architecture).to_string())
                            .collect(),
                        compatibilities,
                        required_configuration: seed
                            .required_configuration
                            .iter()
                            .map(|property| (*property).to_string())
                            .collect(),
                        requires_iam_permissions: seed.requires_iam_permissions,
                    }
                })
                .collect();
            versions.reverse();
            AddonCatalogRecord {
                addon_name: seed.addon_name.to_string(),
                addon_type: seed.addon_type.to_string(),
                owner: seed.owner.to_string(),
                publisher: seed.publisher.to_string(),
                default_namespace: seed.default_namespace.to_string(),
                workload: seed.workload,
                required_addons: seed
                    .required_addons
                    .iter()
                    .map(|required| (*required).to_string())
                    .collect(),
                admission_webhook: seed.admission_webhook,
                marketplace_information: (seed.owner == ADDON_OWNER_MARKETPLACE)
                    .then(|| marketplace_listing(seed.addon_name)),
                pod_identity: seed
                    .pod_identity
                    .iter()
                    .map(|(service_account, policies)| {
                        (
                            (*service_account).to_string(),
                            policies.iter().map(|policy| (*policy).to_string()).collect(),
                        )
                    })
                    .collect(),
                configuration_properties: ADDON_COMMON_CONFIGURATION_PROPERTIES
                    .iter()
                    .chain(seed.configuration_properties)
                    .map(|(property, json_type)| {
                        ((*property).to_string(), (*json_type).to_string())
                    })
                    // A `BTreeMap` so a key the row declares a type of its own
                    // for overrides the common one, and the properties come out
                    // in the schema's alphabetical order with no duplicates.
                    .collect::<BTreeMap<String, String>>()
                    .into_iter()
                    .collect(),
                versions,
            }
        })
        .collect()
}
