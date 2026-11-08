Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-addon-name-and-version-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-addon-name-and-version-mistakes eks-efsfips-traj-configuration-document-mistakes eks-efsfips-traj-marketplace-fips-addon-detour eks-efsfips-traj-reference-install-retry-and-update-while-creating eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateAddon|2xx->4xx|$.addon
Issue-Key: eks|CreateAddon|2xx->4xx|status
Issue-Key: eks|DescribeAddon|2xx->4xx|$
Issue-Key: eks|DescribeAddon|2xx->4xx|$.addon
Issue-Key: eks|DescribeAddon|2xx->4xx|$.addon.status
Issue-Key: eks|DescribeAddon|2xx->4xx|status
Issue-Key: eks|CreateAddon|4xx->4xx|$.message
Issue-Key: eks|CreateAddon|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateAddon|4xx->4xx|$.addonName
Issue-Key: eks|CreateAddon|4xx->4xx|$.clusterName
Issue-Key: eks|DeleteAddon|2xx->4xx|$.addon
Issue-Key: eks|DeleteAddon|2xx->4xx|status
Issue-Key: eks|ListAddons|2xx->2xx|$.addons[*]
Issue-Key: eks|CreateAddon|4xx->4xx|status
Issue-Key: eks|UpdateAddon|4xx->4xx|$.addonName
Issue-Key: eks|UpdateAddon|4xx->4xx|$.clusterName
Issue-Key: eks|UpdateAddon|4xx->4xx|$.message
Issue-Key: eks|UpdateAddon|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|UpdateAddon|4xx->4xx|status
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EKS add-on lifecycle is unreachable: clusters never leave CREATING and add-on records never materialize

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddon

## Description
Ten net-new root causes share one fault: a cluster created through the emulator never advances out of CREATING, so every add-on operation bound to that cluster either refuses with a cluster-not-ACTIVE sentence or finds no add-on at all. Nothing in the EKS add-on surface can be exercised end to end.

This is NOT the already-open issue 20261029T175608Z-eks-3-net-new-root-causes-startinsightsrefresh-response.md (asynchronous EKS state that never settles for Insights refresh) nor 20261022T154613Z-eks-clusters-cannot-be-provisioned-at-all-no.md (VPC/subnet prerequisites): here CreateCluster succeeds, the cluster is addressable, and it is the CREATING -> ACTIVE transition and the Addon state object that are missing. Those issues do not carry these replay keys.

Cause 1 of 10: CreateAddon on a freshly created cluster is refused instead of creating the add-on.
  AWS: CreateAddon is POST /clusters/{clusterName}/addons and returns 200 with the `addon` member of shape Addon, status CREATING (Smithy: CreateAddon -> CreateAddonResponse.addon : Addon, http 200; errors ClientException/InvalidParameterException/InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404). The recording shows 200 with addon.status 'CREATING', addon.addonVersion, addon.health.issues [], addon.namespaceConfig.namespace 'kube-system' and the configurationValues echoed back verbatim.
  Emulator: HTTP 400 InvalidRequestException, message 'Cluster <clusterName> is in CREATING status; it must be ACTIVE to create an add-on.' and no `addon` member. Fixed text of that emulator sentence is 'Cluster ... is in ... status; it must be ACTIVE to create an add-on.'; the variable parts are the cluster name and the status, both taken from state.
  Why: a client that creates a cluster and then installs an add-on — the normal EKS bootstrap sequence — can never get past the first install. It sees a 400 that it must treat as a permanent request error, with no retry that can ever succeed.
  Fix: the cluster state machine (the EKS state store's cluster status progression) must settle CREATING -> ACTIVE the way the other asynchronous EKS resources do, and CreateAddon's handler must then stop short-circuiting.
  Traces: eks-efsfips-traj-addon-name-and-version-mistakes#task#10 (also eks-efsfips-traj-configuration-document-mistakes#task#10, eks-efsfips-traj-marketplace-fips-addon-detour#task#7, #task#11, eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#8).

Cause 2 of 10: DescribeAddon reports the add-on as nonexistent while the recording polls it to DEGRADED.
  AWS: DescribeAddon is GET /clusters/{clusterName}/addons/{addonName}, 200 with `addon` (Smithy: DescribeAddon -> DescribeAddonResponse.addon : Addon). The recording polls until addon.status is 'DEGRADED' with health.issues[0].code 'InsufficientNumberOfReplicas' and message 'The add-on is unhealthy because all deployments have all pods unscheduled no nodes available to schedule pods'.
  Emulator: HTTP 404 ResourceNotFoundException with message 'The requested resource does not exist.' on every poll; the poll is capped at 60s after 7 attempts and addon.status is never produced.
  Why: a client waiting for the add-on to reach a terminal state loops until its own timeout and then mis-reports the add-on as deleted rather than unhealthy.
  Fix: the add-on state in the EKS state store — an Addon record must exist after CreateAddon and progress to a terminal status — plus DescribeAddon's handler lookup.
  Traces: eks-efsfips-traj-addon-name-and-version-mistakes#task#11 (also eks-efsfips-traj-configuration-document-mistakes#task#11).

Cause 3 of 10: the cluster-not-ACTIVE refusal pre-empts configurationValues schema validation.
  AWS: CreateAddon validates the supplied configurationValues document against the add-on's configuration schema and refuses with 400 and the message 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.<rejectedPointer>: is not defined in the schema and the schema does not allow additional properties]'. Fixed text is 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [' ... ': is not defined in the schema and the schema does not allow additional properties]'; the variable part is the JSON pointer of the offending property taken from the request body. Smithy: CreateAddon.configurationValues : String, error InvalidRequestException (400).
  Emulator: HTTP 400 InvalidRequestException with 'Cluster <clusterName> is in CREATING status; it must be ACTIVE to create an add-on.' — the schema is never consulted.
  Why: a client debugging its configuration document gets no information about which property was rejected, so it cannot correct the document.
  Fix: CreateAddon's validation, which must run the configuration-schema check and order it ahead of the cluster-status gate.
  Traces: eks-efsfips-traj-configuration-document-mistakes#task#6 (also #task#7, #task#8).

Cause 4 of 10: the same refusal pre-empts the malformed-document check, and the error payload carries fields AWS leaves absent.
  AWS: when configurationValues is neither valid JSON nor valid YAML, CreateAddon refuses with 400 and 'The ConfigurationValues provided is not in valid YAML or JSON format.' That is fixed text with no variable part, and the recorded error body has no clusterName and no addonName member.
  Emulator: HTTP 400 InvalidRequestException with 'Cluster <clusterName> is in CREATING status; it must be ACTIVE to create an add-on.', and it adds clusterName and addonName members to the error body that AWS does not emit.
  Why: a client cannot distinguish a syntactically broken document from a transient cluster state, and an error body with extra members breaks strict deserializers.
  Fix: CreateAddon's validation (document parse check ordered ahead of the status gate) and its error serializer, which must not decorate the modeled exception with request members.
  Traces: eks-efsfips-traj-configuration-document-mistakes#task#9.

Cause 5 of 10: DescribeAddon 404s for a marketplace add-on the recording shows in CREATE_FAILED.
  AWS: an add-on whose marketplace subscription is missing is still created and reaches status 'CREATE_FAILED' with health.issues[0].code 'AddonSubscriptionNeeded' and message 'You are currently not subscribed to this add-on. To subscribe, visit the AWS Marketplace console, agree to the seller EULA, select the pricing type if required, then re-install the add-on'. DescribeAddon returns that record with 200.
  Emulator: HTTP 404 with no `addon` member.
  Why: a client cannot learn that the failure is a subscription problem — the add-on simply does not exist as far as the emulator is concerned, so the remediation AWS describes is undiscoverable.
  Fix: the add-on state in the EKS state store must record a CREATE_FAILED add-on with its health issue, rather than dropping the record.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#8 (also eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#9, eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#task#8).

Cause 6 of 10: DeleteAddon 404s instead of returning the add-on in DELETING.
  AWS: DeleteAddon is DELETE /clusters/{clusterName}/addons/{addonName} and returns 200 with `addon` whose status is 'DELETING' (Smithy: DeleteAddon -> DeleteAddonResponse.addon : Addon, http 200).
  Emulator: HTTP 404 ResourceNotFoundException, no `addon` member.
  Why: a client cannot perform the delete-then-reinstall rollback at all, and cannot observe the DELETING state it must wait out before reinstalling.
  Fix: DeleteAddon's handler and the add-on state it mutates.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#9 (also eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#task#7, #task#9).

Cause 7 of 10: ListAddons returns an empty list for a cluster that has an add-on installed.
  AWS: ListAddons is GET /clusters/{clusterName}/addons returning `addons` as a StringList of installed add-on names (Smithy: ListAddons -> ListAddonsResponse.addons : StringList). The recording lists the installed add-on name.
  Emulator: HTTP 200 with addons[0] absent — an empty list.
  Why: a client enumerating installed add-ons to decide whether to install or update concludes the cluster is bare and takes the wrong branch.
  Fix: ListAddons' handler reading the per-cluster add-on collection in the EKS state store.
  Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#12 (also eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#10, eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#task#11).

Cause 8 of 10: re-creating an existing add-on returns the wrong status and the wrong error class.
  AWS: CreateAddon for an add-on that already exists refuses with HTTP 409 ResourceInUseException and the message 'Addon already exists.' (Smithy: CreateAddon error ResourceInUseException, httpError 409). That message is entirely fixed text; the recorded error body has no clusterName or addonName member.
  Emulator: HTTP 400 InvalidRequestException with 'Cluster <clusterName> is in CREATING status; it must be ACTIVE to create an add-on.', plus unmodeled clusterName and addonName members in the body.
  Why: a client retrying an install relies on 409/ResourceInUseException to mean 'already installed, proceed'; a 400 is a different retry class entirely and the retry loop fails permanently.
  Fix: CreateAddon's handler (conflict detection against existing add-on state) and its error serializer.
  Traces: eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#11 (also eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#task#10).

Cause 9 of 10: UpdateAddon on an add-on that is still creating returns 404 instead of the 409 in-progress refusal.
  AWS: UpdateAddon refuses an add-on that is not in a settled state with HTTP 409 ResourceInUseException and 'Addon <addonName> cannot be updated as it is currently in <state> state'. Fixed text is 'Addon ' ... ' cannot be updated as it is currently in ' ... ' state'; the variable parts are the add-on name from the request and the add-on's current status from state. The recorded error body also carries clusterName and addonName. Smithy: UpdateAddon error ResourceInUseException 409, ResourceNotFoundException 404.
  Emulator: HTTP 404 ResourceNotFoundException, x-amzn-errortype 'ResourceNotFoundException', message 'The requested resource does not exist.', and clusterName/addonName absent from the body.
  Why: a client distinguishing 'not there yet, retry' (409) from 'never existed' (404) takes the permanent-failure branch and abandons the update.
  Fix: UpdateAddon's handler — it must find the in-flight add-on in state and classify the refusal as ResourceInUseException.
  Traces: eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#12.

Cause 10 of 10: the same cluster-status gate breaks fixture setup, so CreateAddon fails during setup.
  AWS: CreateAddon returns 200 for the setup install.
  Emulator: HTTP 400.
  Why: every scenario that needs a pre-installed add-on cannot be set up, so whole classes of add-on behaviour are untestable against the emulator.
  Fix: same component as Cause 1 — the cluster status progression plus CreateAddon's handler.
  Traces: eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#setup#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — eks-efsfips-traj-addon-name-and-version-mistakes#task#10, CreateAddon: expected status 200 with $.addon populated (addonName 'aws-efs-csi-driver', status 'CREATING', health.issues [], namespaceConfig.namespace 'kube-system', configurationValues echoed); got 400 and $.addon missing. Cluster of 5 affected steps.
Cause 2 — eks-efsfips-traj-addon-name-and-version-mistakes#task#11, DescribeAddon: expected 200 and $.addon.status polled to 'DEGRADED' with health.issues[0].code 'InsufficientNumberOfReplicas'; got 404 {'__type': 'ResourceNotFoundException', 'message': 'The requested resource does not exist.'}; poll capped at 60s, 'not in [DEGRADED] after 7 attempt(s)'.
Cause 3 — eks-efsfips-traj-configuration-document-mistakes#task#6, CreateAddon: expected $.message 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.useFips: is not defined in the schema and the schema does not allow additional properties]'; got 'Cluster sim2real-probe-efsfips-cfg-a9fdcf83 is in CREATING status; it must be ACTIVE to create an add-on.' with x-amzn-errortype 'InvalidRequestException'. 3 affected steps.
Cause 4 — eks-efsfips-traj-configuration-document-mistakes#task#9, CreateAddon: expected $.message 'The ConfigurationValues provided is not in valid YAML or JSON format.' and NO $.clusterName / $.addonName; got the CREATING-status sentence plus $.clusterName and $.addonName populated.
Cause 5 — eks-efsfips-traj-marketplace-fips-addon-detour#task#8, DescribeAddon: expected 200 with $.addon.status 'CREATE_FAILED' and health.issues[0].code 'AddonSubscriptionNeeded'; got 404 with $.addon missing. 3 affected steps.
Cause 6 — eks-efsfips-traj-marketplace-fips-addon-detour#task#9, DeleteAddon: expected 200 with $.addon.status 'DELETING'; got 404. 3 affected steps.
Cause 7 — eks-efsfips-traj-marketplace-fips-addon-detour#task#12, ListAddons: expected $.addons[0] == 'aws-efs-csi-driver'; got None (empty list). 3 affected steps.
Cause 8 — eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#11, CreateAddon: expected status 409 and $.message 'Addon already exists.' with $.clusterName/$.addonName absent; got 400, 'Cluster sim2real-probe-efsfips-ref-9f378d03 is in CREATING status; it must be ACTIVE to create an add-on.', x-amzn-errortype 'InvalidRequestException', and both members present. 2 affected steps.
Cause 9 — eks-efsfips-traj-reference-install-retry-and-update-while-creating#task#12, UpdateAddon: expected status 409 and $.message 'Addon aws-efs-csi-driver cannot be updated as it is currently in CREATING state' with $.clusterName/$.addonName present; got 404, 'The requested resource does not exist.', x-amzn-errortype 'ResourceNotFoundException', both members missing.
Cause 10 — eks-efsfips-traj-rollback-delete-and-reinstall-while-deleting#setup#6, CreateAddon: expected status 200, got 400.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateAddon (POST /clusters/{clusterName}/addons, 200, errors ResourceInUseException 409 / ResourceNotFoundException 404 / InvalidRequestException 400), DescribeAddon (GET /clusters/{clusterName}/addons/{addonName}), DeleteAddon (DELETE, 200, output addon), ListAddons (GET, output addons: StringList), UpdateAddon (POST .../update, errors ResourceInUseException 409).

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1) against it.
3. Inspect fixture eks-efsfips-traj-addon-name-and-version-mistakes step task#10 for the representative divergence, and the fixtures/steps listed per cause above for the remaining nine.
4. Manually: POST /clusters to create a cluster, then immediately POST /clusters/<name>/addons. The emulator answers 400 'Cluster <name> is in CREATING status; it must be ACTIVE to create an add-on.' and keeps answering it however long you wait, because the cluster never transitions to ACTIVE. Every subsequent DescribeAddon / DeleteAddon / UpdateAddon on that add-on answers 404 and ListAddons answers an empty list.
