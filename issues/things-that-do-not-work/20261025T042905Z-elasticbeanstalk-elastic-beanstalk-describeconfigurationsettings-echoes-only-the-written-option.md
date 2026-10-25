Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default|run#26|AssertionError|assert len(resolved) > 1, (
Issue-Commit: 06b7d43ec7937dbf787f77738b3813bde2a779f1

# Elastic Beanstalk DescribeConfigurationSettings echoes only the written option settings instead of the resolved set (platform defaults + overrides), so OptionsToRemove cannot fall back to a default

> _Note: this issue was identified against the code base at commit `06b7d43ec7937dbf787f77738b3813bde2a779f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationSettings

## Description
Cause 1 of 1: configuration option settings are stored and reported as the literal set of options the caller wrote, with no resolution against the solution stack's published option catalog. Failing check: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default.

AWS: Smithy model — DescribeConfigurationSettings (input ApplicationName required, TemplateName len 1..100 / EnvironmentName len 4..40; output ConfigurationSettings: ConfigurationSettingsDescriptionList) "Returns a description of the settings for the specified configuration set, that is, either a configuration template or the configuration set associated with a running environment." Elastic Beanstalk resolves configuration through a layered chain — platform defaults -> saved configuration template -> environment overrides — and the settings description it returns is the RESOLVED set: it contains options the caller never wrote, carrying the DefaultValue that DescribeConfigurationOptions publishes for the same (Namespace, OptionName) for that solution stack. Consequently UpdateConfigurationTemplate's OptionsToRemove ("A list of configuration options to remove from the configuration set", Smithy: UpdateConfigurationTemplateMessage.OptionsToRemove) makes the removed option resolve to the inherited platform default again — it does not vanish from the resolved set and does not become null. Status/error codes are not at issue here; both calls succeed on AWS and on the emulator.

Emulator: DescribeConfigurationSettings returns exactly the written option, i.e. a single-element OptionSettings list. Replay detail: `AssertionError: DescribeConfigurationSettings must return the RESOLVED set (platform defaults plus the one override), not just the written option; got 1 settings: [{'Namespace': 'aws:autoscaling:asg', 'OptionName': 'MinSize', 'Value': '3'}]` — the check had created a template whose only OptionSettings entry was aws:autoscaling:asg/MinSize, and the sibling aws:autoscaling:asg/MaxSize (published with a DefaultValue by DescribeConfigurationOptions for the same stack) is absent from the response. Because the resolution layer does not exist, the second half of the same defect is unreachable by the check but follows directly: with the option set held as the written set only, UpdateConfigurationTemplate(OptionsToRemove=[{aws:autoscaling:asg, MinSize}]) deletes the key and the option disappears from the subsequent DescribeConfigurationSettings response rather than resolving to the catalog DefaultValue.

Why: a client that reads effective configuration through DescribeConfigurationSettings (the EB CLI's `eb config`, drift checks, IaC tools that diff desired vs effective settings) sees an option set that is missing every inherited option, so it concludes options are unset when they are in fact in force at their platform default, and after an OptionsToRemove it sees the option gone rather than back at its default. Both lead a client to re-write defaults explicitly or to mis-report the environment/template as unconfigured.

Fix: the configuration-resolution layer behind the configuration-set read — DescribeConfigurationSettings' handler must project the stored option settings over the solution stack's option catalog (the same catalog DescribeConfigurationOptions serves, including its DefaultValue per option) rather than cloning the stored list, for the template branch and the environment deployed/draft branches alike; and the template/environment state must keep the written overrides distinct from the resolved view so that removing an override re-exposes the default instead of deleting the option.

Traces: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default. Handler that clones the stored list verbatim: services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_configuration_settings.rs lines 203-302 (template branch via crate::state::configuration_settings_description! at 217-220; environment branch option_settings: Some(environment.option_settings.clone()) at 272 and the draft at 297). Not covered by any open issue: the open elasticbeanstalk issues concern control-plane guards (20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md), the deployment substrate (20261023T205650Z-elasticbeanstalk-elastic-beanstalk-deployment-substrate-is-missing-createstoragelocation-s.md), application-version/environment state machines (20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md) and custom platform versions (20261024T015947Z-elasticbeanstalk-elastic-beanstalk-custom-platform-versions-are-not-registered.md); none of them touch configuration resolution or OptionsToRemove.

## Evidence
Check elasticbeanstalk.configuration-template-option-removal-falls-back-to-default, verdict [mismatch].

Expected (AWS): after CreateConfigurationTemplate(ApplicationName=app, TemplateName=tmpl, SolutionStackName=stack, OptionSettings=[{Namespace: aws:autoscaling:asg, OptionName: MinSize, Value: <default+2>}]), DescribeConfigurationSettings(ApplicationName=app, TemplateName=tmpl) returns one ConfigurationSettingsDescription whose OptionSettings is a strict SUPERSET of the written set: aws:autoscaling:asg/MinSize at the override, plus aws:autoscaling:asg/MaxSize (never written) resolving to the DefaultValue that DescribeConfigurationOptions(SolutionStackName=stack) publishes for it, plus the other platform defaults. Then UpdateConfigurationTemplate(OptionsToRemove=[{Namespace: aws:autoscaling:asg, OptionName: MinSize}]) leaves MinSize present in the resolved set at the published DefaultValue.

Actual (emulator): OptionSettings has length 1 and contains only the written option —
  AssertionError: DescribeConfigurationSettings must return the RESOLVED set (platform defaults plus the one override), not just the written option; got 1 settings: [{'Namespace': 'aws:autoscaling:asg', 'OptionName': 'MinSize', 'Value': '3'}]
(raised at check line 136; the check never reaches the OptionsToRemove fallback assertions because the resolved-set assertion fails first). The explicit override itself is correct; everything inherited is missing.

Code: services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_configuration_settings.rs reports the stored list verbatim (template branch lines 217-220, environment branch line 272, draft line 297) and never consults the solution stack's option catalog or its DefaultValues.

## Steps to Reproduce
1. Boot the emulator and point an elasticbeanstalk client at it.
2. stack = one entry from ListAvailableSolutionStacks.
3. DescribeConfigurationOptions(SolutionStackName=stack) and note the DefaultValue published for Namespace 'aws:autoscaling:asg', Name 'MinSize' and for Name 'MaxSize'.
4. CreateApplication(ApplicationName=app).
5. CreateConfigurationTemplate(ApplicationName=app, TemplateName=tmpl, SolutionStackName=stack, OptionSettings=[{Namespace: 'aws:autoscaling:asg', OptionName: 'MinSize', Value: str(int(MinSize default) + 2)}]).
6. DescribeConfigurationSettings(ApplicationName=app, TemplateName=tmpl) -> observe ConfigurationSettings[0].OptionSettings has exactly 1 entry (the written MinSize) and no aws:autoscaling:asg/MaxSize; AWS returns the resolved set including MaxSize at its published DefaultValue.
7. UpdateConfigurationTemplate(ApplicationName=app, TemplateName=tmpl, OptionsToRemove=[{Namespace: 'aws:autoscaling:asg', OptionName: 'MinSize'}]), then DescribeConfigurationSettings again -> observe MinSize absent entirely; AWS resolves it back to the published DefaultValue.
8. Replay: elasticbeanstalk.configuration-template-option-removal-falls-back-to-default.
