Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order eks.create-addon-on-missing-cluster-rejected-before-schema-validation eks.deploy-accessentry-via-cloudformation-grants-access
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order|_cluster_version#4|IndexError|entry = eks.describe_addon_versions(addonName=addon)["addons"][0]
Issue-Key: eks.create-addon-on-missing-cluster-rejected-before-schema-validation|run#7|AssertionError|assert (schema.get("properties") or {}).get("useFIPS", {}).get("type") == "boolean", (
Issue-Key: eks.deploy-accessentry-via-cloudformation-grants-access|_delete_stack#26|Timeout|cfn.get_waiter("stack_delete_complete").wait(
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# eks: 3 net-new root causes — the add-on catalog omits cert-manager and the HyperPod training operator, DescribeAddonConfiguration serves a placeholder schema for aws-efs-csi-driver, and a CloudFormation AWS::EKS::AccessEntry stack never settles

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
Three net-new root causes in the eks emulator. None of them is an unimplemented scaffold stub (every operation in the path answers 200), and none is covered by an open issue: the nearest neighbours are 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md (501 stubs / validation ordering), 20261022T154613Z-eks-clusters-cannot-be-provisioned-at-all-no.md (AWS::EKS::Cluster is a CFN no-op, default-VPC state, EC2 stubs) and 20261023T023215Z-eks-access-entry-not-found-refusals-do-not.md (access-entry not-found message wording). None of them names the add-on catalog, DescribeAddonConfiguration's schema document, or the AWS::EKS::AccessEntry resource type, so they do not cover any cause below. All three causes must be closed.

Cause 1 of 3: the seeded add-on catalog omits add-ons AWS publishes, so DescribeAddonVersions reports an empty listing for a real add-on name.
AWS: Smithy: eks#DescribeAddonVersions, GET /addons/supported-versions, input addonName (httpQuery, String) 'The name of the add-on. The name must match one of the names returned by ListAddons', output addons -> Addons (list of AddonInfo), errors InvalidParameterException/400, ResourceNotFoundException/404, ServerException/500. The operation reports the EKS add-on catalog, which is a publication rather than per-account data; for an add-on AWS publishes, the call returns exactly one AddonInfo whose addonVersions each carry compatibilities naming the Kubernetes cluster versions that build supports (and the defaultVersion flag for one of them). cert-manager and amazon-sagemaker-hyperpod-training-operator are both published EKS add-ons, so DescribeAddonVersions(addonName='cert-manager') and DescribeAddonVersions(addonName='amazon-sagemaker-hyperpod-training-operator') each return a one-element addons list.
Emulator: both calls answer HTTP 200 with addons: [] — an empty listing. The handler is correct and deliberately treats an unknown name as an empty listing rather than an error (see its own module doc: 'a name the catalog does not publish simply matches nothing, which is an EMPTY listing rather than an error'); the defect is the CATALOG SEED, which does not publish these add-ons. The replay therefore dies in the catalog lookup (`entry = eks.describe_addon_versions(addonName=addon)['addons'][0]` -> IndexError: list index out of range) before any CreateAddon assertion runs, so the whole synchronous-validation and ordering contract of the add-on install path is unexercised.
Why: a client cannot discover any published version of these add-ons, cannot pick a Kubernetes cluster version compatible with them, and cannot install them at all — CreateAddon for a name the catalog does not publish is refused InvalidParameterException 'Addon <name> specified is not supported in <version> kubernetes version'. Every catalog-driven workflow (version pinning, default-version resolution, compatibility matrix) silently sees an add-on that does not exist on this service.
Fix: the add-on catalog seed in this service's state (crate::state::AddonCatalogRecord, the seed read by published_addons) must publish the add-ons Amazon EKS publishes — including cert-manager and amazon-sagemaker-hyperpod-training-operator — each with its builds, publisher/owner/type and per-build compatibilities. The DescribeAddonVersions handler's filtering is not the defect.
Traces: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order (the same catalog gap also blocks eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle, filed separately because its check id carries a different service token).

Cause 2 of 3: DescribeAddonConfiguration serves a generic placeholder JSON Schema instead of the add-on's real configuration schema, so aws-efs-csi-driver's documented configuration keys do not exist.
AWS: Smithy: eks#DescribeAddonConfiguration, GET /addons/configuration-schemas, required input addonName + addonVersion, output configurationSchema (String) and podIdentityConfiguration (AddonPodIdentityConfigurationList); the model does not state the schema's contents, so recorded behavior only: configurationSchema is the add-on build's own JSON Schema, and for aws-efs-csi-driver it declares the driver's configurable keys — among them a top-level boolean property `useFIPS`, which is exactly the key the documented FIPS install passes as `--configuration-values '{"useFIPS": true}'`. CreateAddon validates configurationValues against that schema (Smithy: eks#CreateAddon models InvalidParameterException/400 for a document the schema rejects).
Emulator: the configurationSchema returned for aws-efs-csi-driver has only two properties — `namespace` (string, default 'kube-system') and `serviceAccount` (object with a `name` string, default 'efs-csi-controller-sa'). `useFIPS` is absent entirely. The two members present are generic add-on plumbing, not this add-on's schema, so the document is a placeholder shared across add-ons rather than the build's published schema.
Why: a client that reads the schema to discover what it may configure is told the add-on's real options do not exist, and a legal `{"useFIPS": true}` install is either rejected as 'not defined in the schema' or accepted with the key silently dropped. Schema-driven tooling (console forms, CDK/Terraform validation, `aws eks describe-addon-configuration` consumers) derives a wrong configuration surface for every add-on served this placeholder.
Fix: the add-on catalog seed in this service's state must carry each published build's real configurationSchema document, and the DescribeAddonConfiguration handler must serve it verbatim (and CreateAddon must validate configurationValues against that same document).
Traces: eks.create-addon-on-missing-cluster-rejected-before-schema-validation.

Cause 3 of 3: a CloudFormation stack whose only resource is AWS::EKS::AccessEntry never settles, so the access entry it declares is never observable.
AWS: recorded behavior only — a stack containing one AWS::EKS::AccessEntry (ClusterName, PrincipalArn, AccessPolicies=[{PolicyArn: arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy, AccessScope:{Type: cluster}}]) reaches a TERMINAL stack status (CREATE_COMPLETE on success, or CREATE_FAILED/ROLLBACK_COMPLETE carrying a ResourceStatusReason on the AccessEntry logical id); after CREATE_COMPLETE eks:DescribeAccessEntry(clusterName, principalArn) returns the entry and eks:ListAssociatedAccessPolicies lists the granted policy ARN, and after the stack is deleted DescribeAccessEntry answers ResourceNotFoundException with the message 'The requested resource does not exist.' The requirement is that the stack reaches SOME terminal status and that the EKS state agrees with it.
Emulator: the workflow never completes — the check is cut off at the 60 s harness cap with no terminal stack status and no EKS answer ever recorded, so neither the CREATE_COMPLETE leg nor a failure reason is produced. Every other leg of this check (cluster creation, the IAM roles, the principal becoming recognised) is the same plumbing other eks checks in this suite complete inside the budget, which places the hang on the AWS::EKS::AccessEntry provisioning path rather than on cluster setup. Hypothesis: the resource type is unknown to the CFN provisioner and the stack is left in CREATE_IN_PROGRESS indefinitely rather than failing.
Why: a CloudFormation client gets neither success nor failure: `create_stack` returns, every waiter runs to its own cap, and there is no ResourceStatusReason to act on. An IaC-driven access-entry grant can never be confirmed or diagnosed, and the stack status stops being a usable readiness signal.
Fix: the CloudFormation resource provisioning for AWS::EKS::AccessEntry — it must drive the EKS CreateAccessEntry/AssociateAccessPolicy path, delete the entry on stack delete, and reach a terminal CREATE_FAILED with a ResourceStatusReason when it cannot, never leaving the stack non-terminal.
Traces: eks.deploy-accessentry-via-cloudformation-grants-access.

## Evidence
All three failing checks come from suite simparity-behavioral-ga-prod-v1 (30/111 passed). None is marked ALREADY FILED or OWNED in the replay report.

Cause 1 — eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order [error]:
  File check_eks_mbes_addon_install_on_a_live_cluster_rejects_bad_requests_and_bad_order.py line 153, in _cluster_version:
    entry = eks.describe_addon_versions(addonName=addon)['addons'][0]
  IndexError: list index out of range
  Expected: DescribeAddonVersions(addonName='cert-manager') -> addons with exactly one AddonInfo carrying addonVersions[].compatibilities[].clusterVersion (same for addonName='amazon-sagemaker-hyperpod-training-operator').
  Actual: HTTP 200 with addons: [] for both names, so the first index raises. The dependent contract that is consequently never reached (listed as what the fix must unblock, not as separate causes): CreateAddon refusals InvalidParameterException/400 'Addon <name> specified is not supported in <version> kubernetes version', 'Addon version specified is not supported', 'Resolve conflicts option <value> is not supported for operation CreateAddon', 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.<key>: is not defined in the schema and the schema does not allow additional properties]'; ResourceInUseException/409 on a duplicate install; ResourceNotFoundException/404 'The requested resource does not exist.' for DescribeAddon/UpdateAddon/DeleteAddon of an add-on that is not installed. In each of those messages the fixed text is as quoted and the variable parts (<name>, <version>, <value>, <key>) come from the request and the resolved cluster version.
  Code citation: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_addon_versions.rs:215-222 filters crate::state's seeded catalog via guard.published_addons(now); lines 19-23 of the same file document that an unpublished name yields an empty listing — so the seed, not the filter, is the gap.

Cause 2 — eks.create-addon-on-missing-cluster-rejected-before-schema-validation [mismatch]:
  AssertionError: useFIPS is not a boolean property of the schema: {'namespace': {'type': 'string', 'default': 'kube-system', 'description': 'The Kubernetes namespace the add-on is installed into.'}, 'serviceAccount': {'type': 'object', 'properties': {'name': {'type': 'string', 'default': 'efs-csi-controller-sa', 'description': 'The Kubernetes service account the add-on runs as.'}}}}
  Expected: the configurationSchema served for aws-efs-csi-driver at its default version for the default cluster version declares properties.useFIPS with type 'boolean'.
  Actual: the schema's properties are only the two generic members quoted above; useFIPS is absent. The preceding calls all succeeded (DescribeClusterVersions(defaultOnly=True) answered, and DescribeAddonVersions(addonName='aws-efs-csi-driver', kubernetesVersion=<default>) returned exactly one add-on), so the gap is confined to the schema document.
  Also never reached because of this cause (what the fix must unblock, same single cause): CreateAddon against a cluster name that does not exist must be refused ResourceNotFoundException with message 'No cluster found for name: <clusterName>.' for BOTH a schema-valid and a schema-invalid configurationValues document — the cluster is resolved before the configuration document is validated, so it is never InvalidParameterException — and ListAddons/DescribeCluster for that name must still answer ResourceNotFoundException with the same message. Fixed text: 'No cluster found for name: ' + the request's clusterName + '.'.

Cause 3 — eks.deploy-accessentry-via-cloudformation-grants-access [mismatch]:
  Error: Check timed out after 60.0s
  Expected: cfn.create_stack of the one-resource AWS::EKS::AccessEntry template reaches a terminal status; on CREATE_COMPLETE, eks.describe_access_entry(clusterName, principalArn).accessEntry.principalArn equals the role ARN and eks.list_associated_access_policies lists arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy; after delete_stack, describe_access_entry answers ResourceNotFoundException 'The requested resource does not exist.'
  Actual: no terminal stack status and no EKS response was recorded at all before the 60 s cap; the check's own waiters (Delay 15, MaxAttempts 40) never saw the stack leave its non-terminal state and no AccessEntry ResourceStatusReason was produced.

## Steps to Reproduce
Boot the emulator and point boto3/the AWS CLI at it.

Cause 1:
1. eks.describe_addon_versions(addonName='cert-manager') -> expect one AddonInfo with addonVersions[].compatibilities; observe HTTP 200 with addons: [].
2. Repeat with addonName='amazon-sagemaker-hyperpod-training-operator' -> same empty listing.
3. (Blocked by 1-2) create an ACTIVE cluster on the newest STANDARD_SUPPORT version both add-ons support, then drive the CreateAddon refusal classes and the DescribeAddon/UpdateAddon/DeleteAddon not-found classes listed in the evidence.
   Replay: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order

Cause 2:
1. k8s = eks.describe_cluster_versions(defaultOnly=True)['clusterVersions'][0]['clusterVersion'].
2. v = eks.describe_addon_versions(addonName='aws-efs-csi-driver', kubernetesVersion=k8s)['addons'][0]['addonVersions'][0]['addonVersion'].
3. json.loads(eks.describe_addon_configuration(addonName='aws-efs-csi-driver', addonVersion=v)['configurationSchema'])['properties'] -> expect a 'useFIPS' property of type 'boolean'; observe only 'namespace' and 'serviceAccount'.
4. eks.create_addon(clusterName=<a name never created>, addonName='aws-efs-csi-driver', configurationValues='{"useFIPS": true}', resolveConflicts='OVERWRITE') and again with configurationValues='{"useFIPS": "true"}' -> both must be ResourceNotFoundException 'No cluster found for name: <name>.'
   Replay: eks.create-addon-on-missing-cluster-rejected-before-schema-validation

Cause 3:
1. Create a VPC with two AZ-distinct subnets, an eks.amazonaws.com cluster role, and an EKS cluster with authenticationMode API_AND_CONFIG_MAP; wait for ACTIVE.
2. Create a second IAM role to act as the access-entry principal.
3. cfn.create_stack with a template whose single resource is AWS::EKS::AccessEntry {ClusterName, PrincipalArn, AccessPolicies:[{PolicyArn:'arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy', AccessScope:{Type:'cluster'}}]}.
4. Poll cfn.describe_stacks -> expect a terminal StackStatus; observe the stack never leaves its non-terminal status, so the waiter and the check run to their caps.
5. (Blocked by 4) eks.describe_access_entry / eks.list_associated_access_policies, then delete_stack and re-read for ResourceNotFoundException 'The requested resource does not exist.'
   Replay: eks.deploy-accessentry-via-cloudformation-grants-access
