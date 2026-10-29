Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order|run#21|AssertionError|assert got == want, f"each malformed CreateAddon must be refused {want!r}; got {got!r}"
Issue-Commit: cc22e59eeda2de4fd1330e74811c0c72a53dba7a

# eks: 3 net-new root causes in CreateAddon request validation — an add-on name the catalog does not publish is installed, an addonVersion incompatible with the cluster is installed, and the resolveConflicts refusal uses the generic constraint wording

> _Note: this issue was identified against the code base at commit `cc22e59eeda2de4fd1330e74811c0c72a53dba7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddon

## Description
Three independent defects in the `CreateAddon` request-validation layer of the EKS emulator, all observed on one ACTIVE cluster in check `eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order`. All three are in the same service (`eks`) and the same handler, but each is its own root cause and each must be closed separately.

Cause 1 of 3 — an `addonName` the add-on catalog does not publish is ACCEPTED instead of refused.
AWS: Smithy `CreateAddon.addonName` is a plain required `String` with no enum or pattern, and its modeled documentation states "The name of the add-on. The name must match one of the names returned by DescribeAddonVersions." That rule is enforced server-side: a name the catalog for the cluster's Kubernetes version does not publish is refused with the modeled `InvalidParameterException` (httpError 400) and the message `Addon <addonName> specified is not supported in <kubernetesVersion> kubernetes version`. Fixed text: `Addon ` … ` specified is not supported in ` … ` kubernetes version`; variable parts: the request's `addonName`, and the Kubernetes version of the cluster named by the `@httpLabel` `clusterName` (from emulator state, not from the request).
Emulator: the call SUCCEEDS — HTTP 200 with an `Addon` body, and the add-on is written to state. This is deliberate in the handler: its LAYER 4 comment says it "accepts any well-formed add-on name", and `state::default_addon_version(...)` falls back to `state::derive_default_addon_version(cluster_version)` for a name the catalog does not cover, so an unpublished name is installed at a derived version.
Why: a client that typos an add-on name, or asks for one that is not available on its cluster's Kubernetes version, is told the install succeeded. It then polls `DescribeAddon`/`ListAddons` for an add-on real AWS never created, and provisioning code that branches on `InvalidParameterException` to pick a supported add-on never takes that branch. Negative-path tests of add-on selection pass against the emulator and fail against AWS.
Fix: the `CreateAddon` handler's body-validation layer (LAYER 4) must check `addonName` against the published catalog for the cluster's Kubernetes version (the same catalog `DescribeAddonVersions` serves) and return `InvalidParameterException` 400 with the message above, before anything is written.
Traces: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order — key `unknown-addon`: expected `('InvalidParameterException', 400, 'Addon <name> specified is not supported in <version> kubernetes version')`, got `('SUCCEEDED', 200, '')`.

Cause 2 of 3 — an `addonVersion` that the catalog does not list as compatible with the cluster's Kubernetes version is ACCEPTED instead of refused.
AWS: Smithy `CreateAddon.addonVersion` is a plain `String`; its modeled documentation states "The version of the add-on. The version must match one of the versions returned by DescribeAddonVersions." AWS refuses a real catalog version of the add-on whose `compatibilities` do not include the cluster's Kubernetes version with `InvalidParameterException` (400) and the fixed message `Addon version specified is not supported` (no variable parts).
Emulator: the call SUCCEEDS — HTTP 200 and the add-on is stored at the requested version. The handler takes `input.addon_version` verbatim whenever it is non-empty (`let addon_version = input.addon_version … .unwrap_or(default_addon_version)`) and never compares it against the catalog's compatibility list for the cluster version.
Why: a client pinning an add-on version is never told the pin is incompatible with its cluster, so version-compatibility handling (fall back, or upgrade the cluster first) is never exercised, and the emulator reports an add-on running at a build AWS would never install.
Fix: the same body-validation layer must reject an `addonVersion` whose catalog entry does not list the cluster's Kubernetes version among its compatibilities, with `InvalidParameterException` 400 and the exact message above, before the record is minted.
Traces: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order — key `unsupported-version`: expected `('InvalidParameterException', 400, 'Addon version specified is not supported')`, got `('SUCCEEDED', 200, '')`.

Cause 3 of 3 — an invalid `resolveConflicts` value is refused with the generic boundary constraint message instead of EKS's own wording.
AWS: `CreateAddon.resolveConflicts` targets the enum `ResolveConflicts` (values NONE | OVERWRITE | PRESERVE). AWS refuses an off-enum value with `InvalidParameterException` (400) and the message `Resolve conflicts option <value> is not supported for operation CreateAddon` — fixed text `Resolve conflicts option ` … ` is not supported for operation CreateAddon`, with the request's `resolveConflicts` value as the only variable part (the operation name is fixed for this operation).
Emulator: the status (400) and error code (`InvalidParameterException`) are right, but the message is the generic smithy boundary-validation rendering: `1 validation error detected: Value 'BANANA' at 'resolveConflicts' failed to satisfy constraint: Member must satisfy enum value set: [OVERWRITE, NONE, PRESERVE]`. The off-enum value is caught by the boundary validator before the handler runs, so the handler never gets to word the refusal.
Why: the error message is part of the contract. Clients and operators that match on EKS's wording (log scrapers, retry/remediation rules, test assertions) do not match, and the emulator leaks its own validation layer's phrasing and member-path format in place of EKS's.
Fix: `resolveConflicts` must be carried past the boundary validator as an unconstrained value for this operation and refused in the `CreateAddon` handler's validation layer with EKS's wording, alongside the two catalog checks above (same component as causes 1 and 2).
Traces: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order — key `bad-resolve-conflicts`.

No open issue covers these: the open add-on issues in the tracker are lightsail's (`20261027T145933Z-lightsail-deleteinstance-parameter-ignored.md`, `20261029T090822Z-lightsail-enableaddon-wrong-behavior.md`). The earlier eks add-on issues (`20261022T154613Z-eks-createaddon-stub.md`, `20261026T235448Z-eks-3-net-new-root-causes-the-add.md`, `20261027T224548Z-eks-2-net-new-root-causes-in-the.md`) are fixed and concerned the stub, the catalog contents and the install lifecycle, not CreateAddon's request validation; the check now gets past all of those and fails only on the three refusals above. The check's later assertions (404 semantics for DescribeAddon/UpdateAddon/DeleteAddon on an absent add-on, and the 409 duplicate) were never reached, so nothing is claimed about them here.

## Evidence
Failing check: eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order [mismatch]

AssertionError at check line 83 (`assert got == want`), comparing the four malformed CreateAddon calls on an ACTIVE cluster:

Expected (AWS):
  unknown-addon            -> ('InvalidParameterException', 400, 'Addon sim2real-no-such-addon specified is not supported in 1.37 kubernetes version')
  unsupported-version      -> ('InvalidParameterException', 400, 'Addon version specified is not supported')
  bad-resolve-conflicts    -> ('InvalidParameterException', 400, 'Resolve conflicts option BANANA is not supported for operation CreateAddon')
  bad-configuration-values -> ('InvalidParameterException', 400, 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.sim2realNotAKey: is not defined in the schema and the schema does not allow additional properties]')

Actual (emulator):
  unknown-addon            -> ('SUCCEEDED', 200, '')
  unsupported-version      -> ('SUCCEEDED', 200, '')
  bad-resolve-conflicts    -> ('InvalidParameterException', 400, "1 validation error detected: Value 'BANANA' at 'resolveConflicts' failed to satisfy constraint: Member must satisfy enum value set: [OVERWRITE, NONE, PRESERVE]")
  bad-configuration-values -> matches (this one is correct)

(The add-on name, the cluster's Kubernetes version and the bad enum value above are this run's values filling the message templates; the requirement is the template, not these literals.)

Model (smithy_query, services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json):
  CreateAddon input: clusterName (@httpLabel, required), addonName (String, required), addonVersion (String), resolveConflicts (enum ResolveConflicts), configurationValues (String) …
  CreateAddon errors: ClientException 400, InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404, ServerException 500
  addonName doc: "The name must match one of the names returned by DescribeAddonVersions."
  addonVersion doc: "The version must match one of the versions returned by DescribeAddonVersions."
  resolveConflicts doc: "one of: NONE | OVERWRITE | PRESERVE"

Emulator source (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_addon.rs):
  lines 305-311 — "this handler accepts any well-formed add-on name (see LAYER 4 below), so a name or a cluster version the catalog does not cover falls back to the derived form"; `default_addon_version(...).unwrap_or_else(|| state::derive_default_addon_version(&cluster_version))`
  lines 316-320 — `let addon_version = input.addon_version … .unwrap_or(default_addon_version);` with no compatibility check
  lines 322-340 — LAYER 4 validates only `is_addon_name_shaped` and `serviceAccountRoleArn`; there is no catalog membership check and no `resolveConflicts` refusal (the comment states the boundary validator already passed the request)
  lines 687-691 — `resolveConflicts` is deliberately not acted on and not validated in the handler

## Steps to Reproduce
1. Build and boot the emulator from the checkout, then run the behavioral suite; the single net-new failure is check `eks.mbes-addon-install-on-a-live-cluster-rejects-bad-requests-and-bad-order`.
2. Manually: create a VPC with two subnets in EKS-supported AZs, an EKS cluster role, and an EKS cluster at the newest STANDARD_SUPPORT Kubernetes version returned by `DescribeClusterVersions`; wait for status ACTIVE.
3. Cause 1: `eks create-addon --cluster-name <cluster> --addon-name sim2real-no-such-addon`. Expected: HTTP 400 `InvalidParameterException`, message `Addon sim2real-no-such-addon specified is not supported in <cluster's k8s version> kubernetes version`. Observed: HTTP 200 with an `Addon` body, and the add-on then appears in `ListAddons`.
4. Cause 2: pick a real `cert-manager` version from `DescribeAddonVersions` whose `compatibilities` do NOT include the cluster's Kubernetes version, then `eks create-addon --cluster-name <cluster> --addon-name cert-manager --addon-version <that version>`. Expected: HTTP 400 `InvalidParameterException`, message `Addon version specified is not supported`. Observed: HTTP 200 and the add-on stored at that version.
5. Cause 3: `eks create-addon --cluster-name <cluster> --addon-name cert-manager --resolve-conflicts BANANA`. Expected: HTTP 400 `InvalidParameterException`, message `Resolve conflicts option BANANA is not supported for operation CreateAddon`. Observed: HTTP 400 `InvalidParameterException` with the generic boundary message `1 validation error detected: Value 'BANANA' at 'resolveConflicts' failed to satisfy constraint: Member must satisfy enum value set: [OVERWRITE, NONE, PRESERVE]`.
6. After steps 3 and 4, `eks list-addons --cluster-name <cluster>` is non-empty on the emulator; on AWS it is empty because no rejected CreateAddon leaves an add-on behind.
