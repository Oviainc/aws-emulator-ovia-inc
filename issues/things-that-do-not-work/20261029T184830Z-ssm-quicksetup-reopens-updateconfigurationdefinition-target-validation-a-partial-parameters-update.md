Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.update-configuration-definition-triggers-redeployment
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|ValidationException
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# REOPENS UpdateConfigurationDefinition target validation: a partial Parameters update is refused, so a definition update can never trigger a redeployment

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfigurationDefinition

## Description
REOPENS 20261024T063151Z-ssm-quicksetup-updateconfigurationdefinition-validation.md: its fix left the call refused. 1 cause.

Cause 1 of 1: UpdateConfigurationDefinition judges the Parameters map the request carries in ISOLATION, so an update that changes only the tag-targeting parameters of a definition whose stored parameter set already names TargetAccounts/TargetRegions is refused with ValidationException, where AWS accepts it and re-runs the deployment.
AWS: PUT /configurationDefinition/{ManagerArn}/{Id} with Parameters={'TargetType': 'Tags', 'TargetTagKey': '<key>', 'TargetTagValue': '<value>'} against a settled AWSQuickSetupType-SSMHostMgmt definition that was created with TargetAccounts=<account> and TargetRegions=<region> is ACCEPTED (200, empty body) and kicks off a fresh deployment cycle: the manager's Deployment status summary passes back through a transient state (INITIALIZING/DEPLOYING) and re-settles to SUCCEEDED/FAILED. Recorded behavior only for the acceptance — Smithy: UpdateConfigurationDefinitionInput.Parameters (ConfigurationParametersMap) is OPTIONAL and the model states no cross-member deployment-target requirement for this operation; ConfigurationDefinitionInput.Parameters' documentation marks TargetAccounts/TargetOrganizationalUnits and TargetRegions required of a DEFINITION, which this definition still satisfies through its stored values. Hypothesis: AWS merges the update's entries into the stored parameter map (so the target rule is evaluated over the merged set) rather than replacing the whole set; what the recording establishes regardless is that this request must not be refused.
Emulator: 400 ValidationException 'Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.' — raised by update_rejection() in src/ops/update_configuration_definition.rs:112-141, which calls crate::state::deployment_targets_rejection("Parameters", parameters, local_account) on the REQUEST's map alone; the handler's own comment at src/ops/update_configuration_definition.rs:376-387 states the replace-the-whole-set interpretation the refusal follows. Nothing is written, no redeployment happens, and the check errors out at the update call.
Why: the operation's normal use — adjusting one definition parameter on a live manager — is unreachable: a client must re-send the complete parameter map on every update or be refused, and the redeployment an update is supposed to trigger never occurs. A caller following the API as AWS serves it gets a 400 on a valid request, and the emulator's StatusSummaries never show the INITIALIZING -> DEPLOYING -> SUCCEEDED cycle that proves the update took effect.
Fix: UpdateConfigurationDefinition's validation and parameter write in the ssm-quicksetup service crate — evaluate the deployment-target rule over the parameter set the definition will HOLD after the update (the stored map with the request's entries applied), not over the request's map alone, and write the parameters the same way; the shared crate::state::deployment_targets_rejection rule itself stays where it is.
Traces: ssm-quicksetup.update-configuration-definition-triggers-redeployment.

## Evidence
ssm-quicksetup.update-configuration-definition-triggers-redeployment [error]: the check creates an AWSQuickSetupType-SSMHostMgmt manager whose definition Parameters are {'TargetType': 'Tags', 'TargetTagKey': <k>, 'TargetTagValue': <v>, 'TargetAccounts': <account id>, 'TargetRegions': <region>} with both local-deployment roles, waits for the Deployment status summary to settle to SUCCEEDED/FAILED, then calls UpdateConfigurationDefinition(ManagerArn=<arn>, Id=<definition id>, Parameters={'TargetType': 'Tags', 'TargetTagKey': <new k>, 'TargetTagValue': <new v>}).
Expected: 200 with an empty body, followed by the Deployment status summary being observed in INITIALIZING/DEPLOYING and then re-settling (the whole cycle lands within ~20s on real AWS).
Actual: botocore.errorfactory.ValidationException — "An error occurred (ValidationException) when calling the UpdateConfigurationDefinition operation: Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters." The refusal is produced by update_rejection() -> crate::state::deployment_targets_rejection on the request's Parameters map (src/ops/update_configuration_definition.rs:112-141), i.e. by the rule the fixed issue 20261024T063151Z-ssm-quicksetup-updateconfigurationdefinition-validation.md added.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Create the local deployment roles, then POST /configurationManager with one definition {"Type": "AWSQuickSetupType-SSMHostMgmt", "TypeVersion": "<latest>", "Parameters": {"TargetType": "Tags", "TargetTagKey": "<k>", "TargetTagValue": "<v>", "TargetAccounts": "<account id>", "TargetRegions": "<region>"}, "LocalDeploymentAdministrationRoleArn": "<admin role arn>", "LocalDeploymentExecutionRoleName": "<exec role name>"}.
3. Poll GET /configurationManager/{ManagerArn} until the Deployment StatusSummaries entry reports SUCCEEDED or FAILED, and read ConfigurationDefinitions[0].Id.
4. PUT /configurationDefinition/{ManagerArn}/{Id} with {"Parameters": {"TargetType": "Tags", "TargetTagKey": "<new k>", "TargetTagValue": "<new v>"}}.
5. Observe 400 ValidationException 'Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.' instead of 200, and note that the manager's Deployment status summary never re-enters INITIALIZING/DEPLOYING.
