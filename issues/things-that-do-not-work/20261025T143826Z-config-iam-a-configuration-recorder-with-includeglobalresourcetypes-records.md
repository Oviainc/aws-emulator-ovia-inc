Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to
Issue-Key: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to|_await_history_items#13|Timeout|time.sleep(10)
Issue-Commit: 6bde29090da6348595db5cc4873571f558be19c5

# config__iam: a configuration recorder with includeGlobalResourceTypes records no configuration item for AWS::IAM::Policy, so GetResourceConfigHistory never returns an item and its relationships graph is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `6bde29090da6348595db5cc4873571f558be19c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE CAUSE must be closed.

Cause 1 of 1: with a customer-managed configuration recorder created and started with recordingGroup {allSupported: true, includeGlobalResourceTypes: true} and a delivery channel in place, a customer-managed IAM policy created and attached to a role afterwards never produces a configuration item, so GetResourceConfigHistory(resourceType="AWS::IAM::Policy", resourceId=<PolicyId>) never yields one and the item's `relationships` graph and `configuration.attachmentCount` can never be observed.
Failing check id: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to.

AWS: recorded behavior only for the production of the item; the model grounds only the shapes. Smithy (com.amazonaws.configservice#StarlingDoveService): GetResourceConfigHistory takes required resourceType (ResourceType enum) + resourceId (@length 1..768) and returns configurationItems: ConfigurationItemList, with modeled refusals InvalidLimitException / InvalidNextTokenException / InvalidTimeRangeException / NoAvailableConfigurationRecorderException / ResourceNotDiscoveredException / ValidationException, all HTTP 400. ConfigurationItem carries arn, resourceType, resourceId, resourceName, configuration ("A JSON-encoded string that contains the contents for the resource configuration") and relationships: RelationshipList; Relationship carries resourceType, resourceId, resourceName and relationshipName ("The type of relationship with the related resource"). BatchGetResourceConfig returns the same graph as baseConfigurationItems. Against AWS, once the recorder is running and global resource types are in scope, a managed policy attached to exactly one role becomes readable as a configuration item whose configuration blob reports policyName and attachmentCount equal to the number of AWS::IAM::Role relationships the SAME item carries, each naming the attached role's RoleId as resourceId, the role name as resourceName, and relationshipName "Is attached to Role"; a role the policy is NOT attached to does not appear. A second AttachRolePolicy produces a NEW configuration item with attachmentCount 2 and both RoleIds related. Note the check's own header marks these expectations as unreplayed hypotheses H2/H3/H4 — the pinned relationshipName literal "Is attached to Role" is taken from the Config developer guide's own example, so if AWS words it differently that observed string is the requirement and the shape assertions (resourceType / resourceId / relationship count == attachmentCount) do not move.
Emulator: the recorder is accepted and reports recording, but Config's recorded inventory for the policy stays empty. The check's bounded poll on GetResourceConfigHistory (`_await_history_items`, declared budget 600s, 10s interval, treating ResourceNotDiscoveredException as an empty history) never satisfies its predicate `any(json.loads(i["configuration"])["attachmentCount"] == 1 ...)`, and the check is killed by the harness: `Error: Check timed out after 60.0s`. No exception surfaces from the probe — the read path answers successfully with nothing recorded — so a client cannot distinguish "not recorded yet" from "configuration items are not produced at all".
Why: a client relying on AWS's behavior gets a service that hangs instead of answering. Config's configuration item IS the service's output: the `relationships` block is the only place AWS reports the IAM attachment graph it observed, and `configuration.attachmentCount` is the counter a drift/compliance tool cross-checks it against. Any code that waits for Config to observe an IAM policy (change-triggered rules on AWS::IAM::Policy, attachment-drift detection, BatchGetResourceConfig graph walks, resource-timeline UIs) blocks forever rather than failing, and a target that returns [] for every resource type still passes the rest of the corpus — the only other assertion on `relationships` anywhere asserts an EMPTY list — so the gap is otherwise invisible.
Fix: the configuration-recorder/inventory component of the Config emulator — the recorder state machine must mint configuration items for in-scope resource types (including the global IAM types gated by recordingGroup.includeGlobalResourceTypes) into Config's recorded state, the item builder must populate `relationships` from the live IAM attachment graph and derive `configuration.attachmentCount` from that same state rather than independently, and GetResourceConfigHistory / BatchGetResourceConfig must serve them. Belongs in services/service-com-amazonaws-configservice-starlingdoveservice (the recorder/inventory state module plus src/ops/get_resource_config_history.rs and src/ops/batch_get_resource_config.rs).
Related: open issue 20261023T140058Z-config-never-records-or-evaluates-a-resource-created.md records the same absence of recording reached through ListDiscoveredResources and rule evaluation for AWS::S3::Bucket, and open issue 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md records it reached through CloudFormation-provisioned recorders and the S3 delivery leg. Neither carries this check's replay key, and neither covers this surface: this cause is the GLOBAL IAM resource type (AWS::IAM::Policy under includeGlobalResourceTypes=true) read through GetResourceConfigHistory/BatchGetResourceConfig, and the positive `relationships` + attachmentCount consistency claim, which no other open issue asserts. It must be closed for this check too.
Traces: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; 87/408 checks passed, so this is not a bootstrap failure).

Failing check: [mismatch] config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to
Reported detail: `Error: Check timed out after 60.0s`

Expected (AWS): after `_start_recording(..., recording_group={"allSupported": True, "includeGlobalResourceTypes": True})`, CreatePolicy + CreateRole x2 + AttachRolePolicy(attached_role), GetResourceConfigHistory(resourceType="AWS::IAM::Policy", resourceId=<PolicyId>) returns at least one configurationItem within the check's 600s budget, with:
  - item.arn == the live policy ARN, json.loads(item.configuration)["policyName"] == the policy name
  - json.loads(item.configuration)["attachmentCount"] == 1
  - exactly ONE relationship whose resourceType == "AWS::IAM::Role", resourceId == the attached role's RoleId (AROA...), resourceName == that role's name, relationshipName == "Is attached to Role"
  - the co-existing unattached (bystander) role's RoleId absent from item.relationships
  - len(role relationships) == item.configuration.attachmentCount (same item, both derived from one state)
  - after a second AttachRolePolicy, a NEW item with attachmentCount 2 relating BOTH RoleIds, and the same graph through BatchGetResourceConfig.
Fixed text vs request-derived values: "Is attached to Role" and the resourceType literal "AWS::IAM::Role" are AWS's fixed strings; the policy ARN, PolicyId, policyName, RoleIds and role names are values minted by the run and are not part of the requirement.

Actual (emulator): the probe `GetResourceConfigHistory` answers successfully but produces no configuration item for the policy — the predicate `any(json.loads(i["configuration"]).get("attachmentCount") == 1 for i in its)` is never satisfied, `_await_history_items` loops at its 10s interval, and the harness terminates the check at its 60s cap before the check's own 600s deadline (and before `_await_history_items`' diagnostic, which would have printed the empty result plus DescribeConfigurationRecorderStatus, could be raised). No ClientError is raised by the read, so the read surface is reachable and simply never populated.

Corroboration that the emptiness — not the harness cap — is the cause: the suite's sibling Config checks fail identically (`Check timed out after 60.0s`) on the first poll of Config's recorded inventory, as recorded in open issue 20261023T140058Z-config-never-records-or-evaluates-a-resource-created.md.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Create an IAM role trusting config.amazonaws.com and attach arn:aws:iam::aws:policy/service-role/AWS_ConfigRole.
3. Create an S3 delivery bucket carrying the three AWSConfigBucketPermissionsCheck / AWSConfigBucketExistenceCheck / AWSConfigBucketDelivery statements for config.amazonaws.com.
4. config put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<role-arn>,recordingGroup={allSupported=true,includeGlobalResourceTypes=true}
5. config put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<bucket>
6. config start-configuration-recorder --configuration-recorder-name <rec>, then poll config describe-configuration-recorder-status until recording=true.
7. iam create-policy --policy-name <pol> --policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"config:DescribeConfigRules","Resource":"*"}]}'  (note the returned PolicyId, ANPA...).
8. iam create-role twice (an "attached" role and a "bystander" role, both trusting ec2.amazonaws.com); note each RoleId (AROA...).
9. iam attach-role-policy --role-name <attached-role> --policy-arn <policy-arn>; confirm iam list-entities-for-policy --entity-filter Role reports exactly that one role.
10. Poll config get-resource-config-history --resource-type AWS::IAM::Policy --resource-id <PolicyId>: configurationItems stays empty indefinitely (no ResourceNotDiscoveredException, no item), so attachmentCount 1 and the single "Is attached to Role" relationship naming <attached-role>'s RoleId never appear.
11. config batch-get-resource-config --resource-keys resourceType=AWS::IAM::Policy,resourceId=<PolicyId>: no baseConfigurationItems entry either.
12. Replay simparity-behavioral-ga-prod-v1 and re-replay config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to; it times out on the poll in step 10.
