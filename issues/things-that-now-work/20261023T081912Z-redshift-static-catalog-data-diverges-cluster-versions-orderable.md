Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-catalog-describe-orderable-cluster-options
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-catalog-describe-orderable-cluster-options redshift-static-catalog-metadata-reads redshift-catalog-describe-cluster-versions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeOrderableClusterOptions|2xx->2xx|$.DescribeOrderableClusterOptionsResponse.DescribeOrderableClusterOptionsResult.OrderableClusterOptions.OrderableClusterOption[*]
Issue-Key: redshift|DescribeOrderableClusterOptions|2xx->2xx|$.DescribeOrderableClusterOptionsResponse.DescribeOrderableClusterOptionsResult.OrderableClusterOptions.OrderableClusterOption[*].AvailabilityZones.AvailabilityZone[*]
Issue-Key: redshift|DescribeClusterVersions|2xx->2xx|$.DescribeClusterVersionsResponse.DescribeClusterVersionsResult.ClusterVersions.ClusterVersion.Description
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# Redshift static catalog data diverges: cluster versions, orderable options, supported platforms

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrderableClusterOptions

## Description
Three causes, one shared fault: the static catalog the emulator serves for the account-independent Redshift metadata reads is a placeholder rather than AWS's catalog. These operations take no account state (DescribeOrderableClusterOptions and DescribeClusterVersions model no errors at all), so their responses are pure catalog data and are fully reproducible.

Cause 1 of 3 — DescribeOrderableClusterOptions serves the wrong node types and a shorter AZ list (fixture redshift-catalog-describe-orderable-cluster-options, step task#1).
AWS: Smithy: DescribeOrderableClusterOptions returns OrderableClusterOptionsMessage.OrderableClusterOptions, a list of OrderableClusterOption{ClusterVersion, ClusterType, NodeType, AvailabilityZones: AvailabilityZoneList}; it is paginated by Marker/MaxRecords and models no errors. The recording returns a catalog of 20+ entries spanning the current node types (ra3.16xlarge, ra3.4xlarge and others), each with four availability zones (us-west-2a..us-west-2d) and each AvailabilityZone carrying SupportedPlatforms.
Emulator: returns a short placeholder catalog in which every entry has NodeType 'dc2.large' and only three availability zones (us-west-2a..c) with no SupportedPlatforms, so none of the recorded entries is present; every indexed catalog entry diverges.
Why: this is the operation a client calls to discover what it is allowed to order before creating a cluster. A catalog that advertises only a retired node type means a client driven by it can never choose a currently orderable node type, while a client that asks whether a modern node type is orderable is told no. Region/AZ selection built from the AvailabilityZones list is also wrong.
Fix: the static catalog data backing DescribeOrderableClusterOptions in the redshift service crate — serve the recorded node-type/cluster-type/AZ catalog (with the AZ list for the region) instead of a single-node-type placeholder.

Cause 2 of 3 — AvailabilityZone.SupportedPlatforms is never emitted (fixture redshift-static-catalog-metadata-reads, step task#3).
AWS: Smithy: AvailabilityZone is a structure with members Name (String) and SupportedPlatforms (SupportedPlatformsList). The recording populates each AvailabilityZone with SupportedPlatforms containing a SupportedPlatform with Name 'VPC'.
Emulator: each AvailabilityZone is serialized with Name only; SupportedPlatforms is absent, a value_mismatch on every AZ element of every orderable option.
Why: SupportedPlatforms is how a client learns that the AZ only supports VPC-based (rather than EC2-Classic) clusters. With the member absent, a client that branches on platform support reads None and either skips the AZ or falls through to the wrong branch. This is a serializer-level omission of a modeled member and is distinct from cause 1's wrong catalog contents — it would still be missing after the node-type catalog is corrected.
Fix: the AvailabilityZone serializer / catalog model — populate SupportedPlatforms for every AvailabilityZone the catalog emits.

Cause 3 of 3 — DescribeClusterVersions serves a placeholder Description (fixture redshift-catalog-describe-cluster-versions, step task#1; same cluster also covers redshift-static-catalog-metadata-reads#task#1).
AWS: Smithy: ClusterVersion is a structure with ClusterVersion, ClusterParameterGroupFamily and Description ("The description of the cluster version"); DescribeClusterVersions models no errors. The recording returns the engine build description for the version — 'PADB 1.0.49795' — i.e. the Redshift engine build string, not a restatement of the version number.
Emulator: returns 'Cluster version 1.0', a generated placeholder derived from the version field.
Why: Description is the member that identifies which engine build a version corresponds to; a client or operator using it to pin or report an engine build gets a string that carries no build information. Status and shape are otherwise correct, so this is a catalog-content divergence in a modeled response member.
Fix: the static cluster-version catalog backing DescribeClusterVersions — serve the recorded engine build description per cluster version rather than synthesizing one from the version number.

Traces: redshift-catalog-describe-orderable-cluster-options#task#1 (cause 1), redshift-static-catalog-metadata-reads#task#3 (cause 2), redshift-catalog-describe-cluster-versions#task#1 (cause 3; cluster also covers redshift-static-catalog-metadata-reads#task#1). All three must be closed before submitting.

## Evidence
Cause 1 — redshift-catalog-describe-orderable-cluster-options#task#1, DescribeOrderableClusterOptions, expected 200 / actual 200. Replay detail: "OrderableClusterOption[0]: missing (expected {'ClusterType': 'multi-node', 'AvailabilityZones': {'AvailabilityZone': [{'Name': 'us-west-2a', 'SupportedPlatforms': {'SupportedPlatform': {'Name': 'VPC'}}}, {'Name': 'us-west-2b', ...}, {'Name': 'us-west-2c', ...}, {'Name': 'us-west-2d', ...}]}, 'NodeType': 'ra3.16xlarge', 'ClusterVersion': '1.0'}, got {'ClusterVersion': '1.0', 'ClusterType': 'multi-node', 'NodeType': 'dc2.large', 'AvailabilityZones': {'AvailabilityZone': [{'Name': 'us-west-2a'}, {'Name': 'us-west-2b'}, {'Name': 'us-west-2c'}]}})"; "OrderableClusterOption[1]: missing (expected ... 'NodeType': 'ra3.4xlarge' ..., got ... 'NodeType': 'dc2.large' ...)". The divergence key lists OrderableClusterOption[0] through [19] as all divergent.

Cause 2 — redshift-static-catalog-metadata-reads#task#3, DescribeOrderableClusterOptions, expected 200 / actual 200. Replay detail: "OrderableClusterOption[0].AvailabilityZones.AvailabilityZone[0]: value_mismatch (expected {'Name': 'us-west-2a', 'SupportedPlatforms': {'SupportedPlatform': {'Name': 'VPC'}}}, got {'Name': 'us-west-2a'})" and the same for OrderableClusterOption[1].AvailabilityZones.AvailabilityZone[0].

Cause 3 — redshift-catalog-describe-cluster-versions#task#1, DescribeClusterVersions, expected 200 / actual 200, 1 further affected step (redshift-static-catalog-metadata-reads#task#1). Replay detail: "$.DescribeClusterVersionsResponse.DescribeClusterVersionsResult.ClusterVersions.ClusterVersion.Description: value_mismatch (expected 'PADB 1.0.49795', got 'Cluster version 1.0')".

Model citations: Smithy DescribeOrderableClusterOptions output OrderableClusterOptionsMessage{OrderableClusterOptions, Marker}, errors []; OrderableClusterOption{ClusterVersion, ClusterType, NodeType, AvailabilityZones: AvailabilityZoneList}; AvailabilityZone{Name, SupportedPlatforms: SupportedPlatformsList}; ClusterVersion{ClusterVersion, ClusterParameterGroupFamily, Description}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture redshift-catalog-describe-orderable-cluster-options step task#1 — DescribeOrderableClusterOptions with no filters. Observe every OrderableClusterOption has NodeType 'dc2.large' and three AZs; the recording has 20+ entries across ra3.16xlarge / ra3.4xlarge and other node types with four AZs.
3. Cause 2: replay fixture redshift-static-catalog-metadata-reads step task#3 — DescribeOrderableClusterOptions and inspect each AvailabilityZones.AvailabilityZone element: Name is present, SupportedPlatforms (SupportedPlatform Name 'VPC') is absent.
4. Cause 3: replay fixture redshift-catalog-describe-cluster-versions step task#1 — DescribeClusterVersions and read ClusterVersions.ClusterVersion.Description: observe 'Cluster version 1.0' instead of the recorded engine build string. Reproduces again at redshift-static-catalog-metadata-reads#task#1.
