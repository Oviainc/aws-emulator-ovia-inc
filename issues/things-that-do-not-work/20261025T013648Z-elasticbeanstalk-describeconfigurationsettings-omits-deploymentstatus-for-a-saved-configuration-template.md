Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default|run#20|KeyError|assert record["DeploymentStatus"] == "deployed", (
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# DescribeConfigurationSettings omits DeploymentStatus for a saved configuration template record

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationSettings

## Description
Cause 1 of 1: a template-scoped ConfigurationSettingsDescription is returned without its DeploymentStatus member, so the field is absent from the response rather than carrying the status of the configuration set. Failing check: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default.

AWS: Smithy — DescribeConfigurationSettings returns ConfigurationSettingsDescriptionList of ConfigurationSettingsDescription, whose member DeploymentStatus targets the enum ConfigurationDeploymentStatus with values [deployed, pending, failed]; the same structure is the output shape of CreateConfigurationTemplate. The structure also carries DateCreated (CreationDate) and DateUpdated (UpdateDate). Recorded behavior: for a configuration set that is a SAVED CONFIGURATION TEMPLATE (described by ApplicationName + TemplateName, with no EnvironmentName on the record), the described record carries DeploymentStatus = "deployed". The replayed check asserts exactly that, and asserts the same record carries non-null DateCreated and DateUpdated and no EnvironmentName.

Emulator: the record is returned and its TemplateName, ApplicationName and SolutionStackName match (those assertions pass), but DeploymentStatus is not present at all — the check raises `KeyError: 'DeploymentStatus'` on `record["DeploymentStatus"]`. The environment-scoped branch of the handler does set deployment_status (DEPLOYMENT_STATUS_DEPLOYED for the deployed set, the draft's own status for the draft), so the omission is specific to the template branch, which builds its record from the stored template via `crate::state::configuration_settings_description!` — the stored template record has no deployment status to report. HTTP status is 200 in both cases; this is a response-shape divergence, not a refusal.

Why: a client that reads DeploymentStatus to tell a deployed configuration set from a pending or failed draft gets a missing key / null instead of a modeled enum value. In botocore the member is simply absent from the parsed dict, so any code indexing it (as this check does) raises KeyError rather than branching; code that compares it treats a real template as if it were "not associated with a running environment". Tools that round-trip DescribeConfigurationSettings output back into CreateConfigurationTemplate/UpdateConfigurationTemplate lose the field entirely.

Fix: the template branch of the DescribeConfigurationSettings handler and the stored configuration-template record it is projected from — the projection must populate DeploymentStatus for a template-scoped configuration set the way the environment-scoped branch already populates it, and the stored template must carry the status (plus its DateCreated/DateUpdated instants) for the projection to report.

Note: the check continues past this point to assert that DescribeConfigurationSettings returns the RESOLVED option set (platform defaults as a strict superset of the written overrides, with DefaultValue from DescribeConfigurationOptions) and that UpdateConfigurationTemplate(OptionsToRemove=...) makes an option fall back to the published default instead of disappearing. Those assertions were never reached in this replay, so no claim is made about them here; re-replaying this check id after the DeploymentStatus fix is what shows whether they hold. No currently open elasticbeanstalk issue covers this record field: the configuration-template surface is not part of 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md, 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-deployment-substrate-is-missing-createstoragelocation-s.md, 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md or 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-custom-platform-versions-are-not-registered.md.

Traces: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default (check_elasticbeanstalk_configuration_template_option_removal_falls_back_to_default.py line 120). Emulator code: services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_configuration_settings.rs lines 206-221 (template branch, projection via configuration_settings_description!) versus lines 249-273 (environment branch, which does set deployment_status).

## Evidence
Expected (AWS / Smithy): DescribeConfigurationSettings(ApplicationName=<app>, TemplateName=<tmpl>) -> 200, ConfigurationSettings[0] contains DeploymentStatus with a ConfigurationDeploymentStatus enum value; for a saved template the recorded value is "deployed". The same record carries DateCreated and DateUpdated and no EnvironmentName.

Actual (emulator): 200, ConfigurationSettings[0] has TemplateName/ApplicationName/SolutionStackName correct but NO DeploymentStatus member. Replay output:

  File "check_elasticbeanstalk_configuration_template_option_removal_falls_back_to_default.py", line 120, in run
    assert record["DeploymentStatus"] == "deployed", (
  KeyError: 'DeploymentStatus'

The check's own failure text for this assertion: "a saved configuration template reports DeploymentStatus 'deployed', got <value>". The environment-scoped branch of the same emulator operation does emit deployment_status, so the two configuration-set kinds answer with different shapes.

## Steps to Reproduce
1. elasticbeanstalk.create_application(ApplicationName=<app>)
2. stack = first entry of elasticbeanstalk.list_available_solution_stacks()["SolutionStacks"] (the check prefers an "Amazon Linux 2023 ... running Docker/Python" stack)
3. elasticbeanstalk.create_configuration_template(ApplicationName=<app>, TemplateName=<tmpl>, SolutionStackName=stack, OptionSettings=[{"Namespace": "aws:autoscaling:asg", "OptionName": "MinSize", "Value": <default+2>}])
4. record = elasticbeanstalk.describe_configuration_settings(ApplicationName=<app>, TemplateName=<tmpl>)["ConfigurationSettings"][0]
5. Read record["DeploymentStatus"] -> KeyError on the emulator; against AWS it is "deployed". (record["TemplateName"], record["ApplicationName"] and record["SolutionStackName"] all match, so the record itself is returned.)

Replay the whole check with: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default
