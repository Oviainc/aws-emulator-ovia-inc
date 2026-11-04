Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-catalog-and-schema-discovery
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-catalog-and-schema-discovery eks-ekscc-traj-lenient-scalar-coercion eks-ekscc-traj-reference-create-retry-and-delete
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonConfiguration|2xx->2xx|$.podIdentityConfiguration[*]
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.vendedLogs
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.vendedLogs
Issue-Related: 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# EKS responses omit fields AWS returns: Cluster.vendedLogs and add-on configuration schema content

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonConfiguration

## Description
Three net-new root causes share one fault: EKS success responses (HTTP 200) are serialized from data that does not match what AWS returns for the same request — the add-on configuration-schema catalog serves the wrong document and a short podIdentityConfiguration list, and the Cluster object returned by the cluster lifecycle operations drops a member AWS always emits. Status codes are correct in all three; only the response body diverges.

Related already-open issues, named here because they do NOT carry these replay keys: 20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md owns DescribeAddonConfiguration's 200->404 and 400->501 routing defects, and 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md owns the clusters where a catalog response is simply EMPTY. Cause 1 below is a different failure mode: the operation answers 200 with a populated but WRONG schema document.

Cause 1 of 3: DescribeAddonConfiguration answers 200 with a configuration schema that belongs to a different add-on, and with an incomplete podIdentityConfiguration list.
  AWS: DescribeAddonConfiguration is GET /addons/configuration-schemas with addonName and addonVersion both @required httpQuery parameters, returning 200 with addonName, addonVersion, configurationSchema (the JSON-Schema document published for that exact add-on name + version) and podIdentityConfiguration, a list of AddonPodIdentityConfiguration (members serviceAccount, recommendedManagedPolicies) — one entry per Kubernetes service account the add-on uses (Smithy: DescribeAddonConfiguration -> DescribeAddonConfigurationResponse.{addonName,addonVersion,configurationSchema,podIdentityConfiguration: AddonPodIdentityConfigurationList}; shape AddonPodIdentityConfiguration). For the recorded aws-efs-csi-driver version the published schema is a draft 2019-09 document ($schema "https://json-schema.org/draft/2019-09/schema", additionalProperties false, description "Configurable parameters for the aws-efs-csi-driver") whose top-level properties are controller, node and useFIPS, and the response carries more than one podIdentityConfiguration entry.
  Emulator: answers 200 but serves a draft-06 document ($schema "http://json-schema.org/draft-06/schema#", a generic/other-add-on schema) in configurationSchema, so the properties the client must fill in (useFIPS, controller.*, node.*) are not described at all; and podIdentityConfiguration is served one entry short — the element at index 1 is absent.
  Why: a client fetches this schema specifically to validate the configurationValues document it is about to pass to CreateAddon/UpdateAddon. Validating against the wrong schema either rejects a key AWS accepts (useFIPS) or accepts keys AWS rejects, so the client's add-on install fails only at CreateAddon time. A short podIdentityConfiguration list hides a service account whose IAM policy the client must pre-create, so pod identity is left unconfigured.
  Fix: the add-on configuration-schema catalog data behind DescribeAddonConfiguration's handler — the schema document and the podIdentityConfiguration list must be keyed by (addonName, addonVersion) rather than falling back to a shared/default schema.
  Traces: eks-efsfips-traj-catalog-and-schema-discovery#task#1.

Cause 2 of 3: CreateCluster's 200 response omits cluster.vendedLogs.
  AWS: recorded behavior only — the on-disk Smithy model's Cluster shape does not declare a vendedLogs member, but the recorded AWS CreateCluster 200 response includes cluster.vendedLogs as a boolean, present on every cluster and false for a cluster created without vended log delivery. It is a plain member of the Cluster object alongside name/arn/status/logging, not a conditional one.
  Emulator: the member is absent from the serialized Cluster object entirely (JSON key not emitted), for an otherwise successful 200 CreateCluster.
  Why: a client that reads cluster.vendedLogs to decide whether vended logging is on sees an absent member rather than false. In a typed SDK the field deserializes to null/None instead of false, so code that branches on it takes the "unknown" path or raises on a null unboxing; code that round-trips the Cluster object back into a comparison or a template loses the field.
  Fix: the Cluster serializer / cluster state model shared by the cluster lifecycle operations — vendedLogs must be populated (default false) and emitted whenever a Cluster object is serialized.
  Traces: eks-ekscc-traj-lenient-scalar-coercion#task#5 (also eks-ekscc-traj-reference-create-retry-and-delete#task#5).

Cause 3 of 3: DeleteCluster's 200 response omits cluster.vendedLogs the same way.
  AWS: recorded behavior only — DeleteCluster returns 200 with the same Cluster object (in DELETING status), and that object likewise carries vendedLogs as a boolean (false for the recorded cluster).
  Emulator: the member is absent from the Cluster object in the DeleteCluster 200 response.
  Why: same as cause 2, on the delete path: a client that re-reads the returned Cluster to confirm the cluster's configuration at deletion time gets a null where AWS gives a boolean. It also shows the omission is in the shared Cluster serialization, not in one handler, so every operation returning a Cluster is affected.
  Fix: the same Cluster serializer / cluster state model, exercised from DeleteCluster's handler.
  Traces: eks-ekscc-traj-lenient-scalar-coercion#task#6 (also eks-ekscc-traj-reference-create-retry-and-delete#task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, bootstrap_signal false, 393 steps replayed).

Cause 1 — eks-efsfips-traj-catalog-and-schema-discovery#task#1, DescribeAddonConfiguration, status 200 -> 200, divergence key eks|DescribeAddonConfiguration|status=200->200|keys=$.configurationSchema,$.podIdentityConfiguration[1]: $.configurationSchema value_mismatch — expected a document beginning '{"$schema":"https://json-schema.org/draft/2019-09/schema","additionalProperties":false,"description":"Configurable parameters for the aws-efs-csi-driver","properties":{"controller":{...},"node":{...},"useFIPS":{"description":"Assert whether or not the driver uses FIPS enabled endpoints","type":"boolean"}},"type":"object"}'; got a document beginning '{"$schema":"http://json-schema.org/draft-06/schema#","ty...'. $.podIdentityConfiguration[1] diverges — the expected second list element is not served. 1 affected step.

Cause 2 — eks-ekscc-traj-lenient-scalar-coercion#task#5, CreateCluster, status 200 -> 200, divergence key eks|CreateCluster|status=200->200|keys=$.cluster.vendedLogs: '$.cluster.vendedLogs: missing (expected False, got None)'. 2 affected steps (also eks-ekscc-traj-reference-create-retry-and-delete#task#5).

Cause 3 — eks-ekscc-traj-lenient-scalar-coercion#task#6, DeleteCluster, status 200 -> 200, divergence key eks|DeleteCluster|status=200->200|keys=$.cluster.vendedLogs: '$.cluster.vendedLogs: missing (expected False, got None)'. 2 affected steps (also eks-ekscc-traj-reference-create-retry-and-delete#task#7).

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DescribeAddonConfiguration (GET /addons/configuration-schemas, code 200, input addonName/addonVersion both httpQuery + required, output addonName, addonVersion, configurationSchema, podIdentityConfiguration: AddonPodIdentityConfigurationList, errors InvalidParameterException 400 / ResourceNotFoundException 404 / ServerException 500); shape AddonPodIdentityConfiguration (serviceAccount, recommendedManagedPolicies); shape Cluster — vendedLogs is NOT declared in the model, so causes 2 and 3 rest on the recording only.

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-efsfips-traj-catalog-and-schema-discovery, step task#1. The other two causes: eks-ekscc-traj-lenient-scalar-coercion#task#5 (CreateCluster) and eks-ekscc-traj-lenient-scalar-coercion#task#6 (DeleteCluster).
4. Manually, cause 1: GET /addons/configuration-schemas?addonName=aws-efs-csi-driver&addonVersion=<the recorded version> -> 200, then compare the returned configurationSchema's $schema and top-level properties against the add-on's published schema (expect draft 2019-09 with controller/node/useFIPS; the emulator returns a draft-06 document), and count the podIdentityConfiguration entries (the emulator returns one fewer).
5. Manually, causes 2 and 3: POST /clusters to create a cluster -> 200 and inspect the returned cluster object; then DELETE /clusters/<name> -> 200 and inspect it again. In both responses the vendedLogs key is absent; AWS emits vendedLogs: false.
