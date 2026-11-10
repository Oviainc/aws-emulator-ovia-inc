Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-configuration-template-lifecycle-and-validation
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-configuration-template-lifecycle-and-validation elasticbeanstalk-environment-create-option-setting-reflected elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|CreateConfigurationTemplate|2xx->2xx|$.CreateConfigurationTemplateResponse.CreateConfigurationTemplateResult.TemplateUrl
Issue-Key: elasticbeanstalk|UpdateConfigurationTemplate|2xx->2xx|$.UpdateConfigurationTemplateResponse.UpdateConfigurationTemplateResult.TemplateUrl
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.EnvironmentLinkPoints
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*]
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.Tier
Issue-Key: elasticbeanstalk|DescribeConfigurationOptions|2xx->2xx|$.DescribeConfigurationOptionsResponse.DescribeConfigurationOptionsResult.Options.member[*].Description
Issue-Key: elasticbeanstalk|DescribeConfigurationOptions|2xx->2xx|$.DescribeConfigurationOptionsResponse.DescribeConfigurationOptionsResult.Options.member[*].MaxValue
Issue-Key: elasticbeanstalk|DescribeConfigurationOptions|2xx->2xx|$.DescribeConfigurationOptionsResponse.DescribeConfigurationOptionsResult.Options.member[*].UserDefined
Issue-Key: elasticbeanstalk|DescribeConfigurationOptions|2xx->2xx|$.DescribeConfigurationOptionsResponse.DescribeConfigurationOptionsResult.Tier
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].Namespace
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].OptionName
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].ResourceName
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].Value
Issue-Key: elasticbeanstalk|DescribeAccountAttributes|2xx->2xx|$.DescribeAccountAttributesResponse.DescribeAccountAttributesResult.ResourceQuotas.HealthGroupQuota
Issue-Key: elasticbeanstalk|CreateEnvironment|2xx->2xx|$.CreateEnvironmentResponse.CreateEnvironmentResult.EnvironmentId
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# Elastic Beanstalk configuration, quota and environment responses drop recorded members (7 causes)

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationTemplate

## Description
SEVEN distinct root causes in the elasticbeanstalk emulator, all in the same service: successful (HTTP 200) responses whose bodies are missing members AWS's wire response carries, or whose generated identifier does not have AWS's shape. In every cause the status matches, so a client sees a success and silently reads a short body. Several of these members (TemplateUrl, OptionSettingDetails, EnvironmentLinkPoints, the Tier member of the configuration responses, ConfigurationOptionDescription.Description, ResourceQuotas.HealthGroupQuota) are NOT in the Smithy model — they are documented here as recorded behavior only, from the awsQuery wire responses in the corpus. SEVEN causes must be closed before submitting.

Cause 1 of 7: CreateConfigurationTemplate omits TemplateUrl.
AWS: Smithy CreateConfigurationTemplate(ApplicationName required, TemplateName required, SolutionStackName | PlatformArn | SourceConfiguration | EnvironmentId, OptionSettings, Tags) -> ConfigurationSettingsDescription. Recorded behavior only: the awsQuery CreateConfigurationTemplateResult additionally carries a TemplateUrl element — the S3 URL of the stored template, built as https://s3-<region>.amazonaws.com/elasticbeanstalk-<region>-<accountId>/resources/templates/<ApplicationName>/<TemplateName>. The fixed parts are the host form, the 'elasticbeanstalk-<region>-<accountId>' bucket form and the '/resources/templates/' path; the variable parts are the region, the account id, and the ApplicationName and TemplateName from the request.
Emulator: the member is absent from the response (got None).
Why: a client that stores or renders the template's S3 location after creating it gets nothing, and any tooling that fetches the template document from that URL has no URL to fetch.
Fix: the response projection of the CreateConfigurationTemplate handler plus the storage-location derivation it needs — services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/create_configuration_template.rs and the configuration-template record in crate::state.
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#task#5.

Cause 2 of 7: UpdateConfigurationTemplate omits TemplateUrl.
AWS: Smithy UpdateConfigurationTemplate(ApplicationName required, TemplateName required, Description, OptionSettings, OptionsToRemove) -> ConfigurationSettingsDescription. Recorded behavior only: the result carries the same TemplateUrl element, with the same template as cause 1, for the template just updated.
Emulator: the member is absent (got None).
Why: same as cause 1, on the update path — a round-trip of create-then-update loses the location both times, so a client cannot discover it at all.
Fix: the response projection of the UpdateConfigurationTemplate handler — src/ops/update_configuration_template.rs (sharing the derivation added for cause 1).
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#task#8.

Cause 3 of 7: DescribeConfigurationSettings for a TEMPLATE omits OptionSettingDetails, EnvironmentLinkPoints and Tier and serves a different OptionSettings set.
AWS: Smithy DescribeConfigurationSettings(ApplicationName required, TemplateName | EnvironmentName) -> ConfigurationSettingsDescriptions{ConfigurationSettings: list of ConfigurationSettingsDescription}. Recorded behavior only: each ConfigurationSettings member additionally carries (a) OptionSettingDetails — one entry per described option with Namespace, OptionName and an OptionSource{Identifier, Origin} naming where the effective value came from ('Service' for a platform default, 'API' for a value the request set), (b) an EnvironmentLinkPoints element (empty for a template), and (c) a Tier element. The OptionSettings list itself is the resolved set: every option the platform version defines a default for, at that default, overlaid with the settings the configuration holds.
Emulator: OptionSettingDetails, EnvironmentLinkPoints and Tier are all absent, and the OptionSettings list it does serve diverges from the recording across ~100 member positions.
Why: OptionSettingDetails/OptionSource is the only way a client can tell a value it set from a platform default, which is exactly what console-like and drift-detecting tooling renders; with the member missing, every setting looks un-attributed, and the resolved OptionSettings set a client diffs against is wrong.
Fix: the ConfigurationSettingsDescription projection in the DescribeConfigurationSettings handler and the option-catalog resolution behind it — src/ops/describe_configuration_settings.rs with the option catalog in crate::state.
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#task#6 (same cluster also fails at task#9).

Cause 4 of 7: DescribeConfigurationSettings for an ENVIRONMENT drops the same members and does not attribute API-set options.
AWS: as cause 3; recorded behavior only: for an environment created with explicit OptionSettings, the OptionSettingDetails entry for each such option carries OptionSource{Identifier: 'API', Origin: 'API'} while platform defaults carry 'Service', and the member also carries EnvironmentLinkPoints and Tier.
Emulator: OptionSettingDetails, EnvironmentLinkPoints and Tier are absent for the environment read too, and the OptionSettings list diverges in Namespace/OptionName/ResourceName/Value across most positions — so the option the CreateEnvironment request set is not reflected in the read the way the recording shows.
Why: this is the read a client uses to confirm an option setting it just applied actually took effect and came from the API rather than the platform; with no OptionSource and a different option set, that confirmation is impossible.
Fix: the same projection and resolution as cause 3, on the environment branch of the handler — src/ops/describe_configuration_settings.rs.
Traces: elasticbeanstalk-environment-create-option-setting-reflected#task#5.

Cause 5 of 7: DescribeConfigurationOptions omits Tier and each option's Description, and publishes wrong MaxValue/UserDefined.
AWS: Smithy DescribeConfigurationOptions(ApplicationName|TemplateName|EnvironmentName|SolutionStackName|PlatformArn, Options) -> ConfigurationOptionsDescription{SolutionStackName, PlatformArn, Options: ConfigurationOptionDescription{Namespace, Name, DefaultValue, ChangeSeverity, UserDefined, ValueType, ValueOptions, MinValue, MaxValue, MaxLength, Regex}}. ConfigurationOptionDescription.UserDefined is documented as 'true: This configuration option was defined by the user ... false: This configuration was not defined by the user', with 'Constraint: You can remove only UserDefined options from a configuration'. MaxValue is 'If specified, the configuration option must be a numeric value less than this value'. Recorded behavior: the result additionally carries a Tier element {Name 'WebServer', Type 'Standard', Version '1.0'} (not modelled), and each option description carries a Description element (not modelled) — for aws:autoscaling:asg MaxSize/MinSize the recorded text is 'Maximum number of instances you want running. Must be a positive integer that is less than 10000.' and 'Minimum number of instances you want running. Must be a positive integer no greater than MaxSize.' — with MaxValue 10000 and UserDefined false, because these are platform-published options.
Emulator: Tier is absent, Description is absent on every option, MaxValue is 20 instead of 10000, and UserDefined is true instead of false for platform-published options.
Why: UserDefined is the flag a client reads to decide what it may legally pass in OptionsToRemove, so marking platform options user-defined invites calls AWS rejects; a MaxValue of 20 makes the emulator reject (or clients pre-reject) valid ASG sizes up to 9999; and the missing Description/Tier leave catalog-rendering tooling with no text and no tier context. The UserDefined half of this cause is ADJACENT to cause 4 of the open behavioral issue 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-accepts-inputs-aws-refuses-and-publishes.md (which covers the aws:elasticbeanstalk:environment/EnvironmentType option), but that issue does not carry this fixture step and the Tier/Description/MaxValue divergences are not in it.
Fix: the configuration-option catalog and the ConfigurationOptionsDescription projection in the DescribeConfigurationOptions handler — src/ops/describe_configuration_options.rs and the option catalog in crate::state.
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#task#7.

Cause 6 of 7: DescribeAccountAttributes omits a resource quota.
AWS: Smithy DescribeAccountAttributes() -> DescribeAccountAttributesResult{ResourceQuotas}; ResourceQuotas models ApplicationQuota, ApplicationVersionQuota, EnvironmentQuota, ConfigurationTemplateQuota and CustomPlatformQuota, each a ResourceQuota{Maximum}. Recorded behavior only: the wire ResourceQuotas additionally carries a HealthGroupQuota element with its own Maximum.
Emulator: HealthGroupQuota is absent from ResourceQuotas (got None).
Why: a client that enumerates the account's Elastic Beanstalk quotas — or looks up this specific quota before creating health groups — finds the entry missing and cannot tell 'no quota' from 'quota unknown'. Only the presence of the member and its Maximum are the requirement; the numeric limit is account data the fixture does not assert.
Fix: the ResourceQuotas projection in the DescribeAccountAttributes handler — src/ops/describe_account_attributes.rs and the account-quota defaults in crate::state.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#1.

Cause 7 of 7: CreateEnvironment mints an EnvironmentId that does not have AWS's shape.
AWS: Smithy CreateEnvironment -> EnvironmentDescription{EnvironmentId: EnvironmentId (string), ...}; the model puts no pattern on it, so recorded behavior only: AWS mints the id as the literal prefix 'e-' followed by a short fixed-width token of lowercase alphanumeric characters (10 in the recording). The fixture asserts the SHAPE, not the value — the id itself is generated and differs per run.
Emulator: it returns 'e-' followed by a 13-character token ('e-321524e501824' in this run), which fails that shape assertion.
Why: EnvironmentId is echoed into ARNs, logs and client-side id validation; a client or test that validates the AWS id shape before using it rejects the emulator's id, and anything that stores ids in a fixed-width column truncates them.
Fix: the EnvironmentId generator used by the CreateEnvironment handler — src/ops/create_environment.rs / the id minting helper in crate::state.
Traces: elasticbeanstalk-environment-create-option-setting-reflected#task#4.

## Evidence
Every cluster below replayed expected 200 / actual 200 — the status matched and only the body diverged.
Cause 1 (elasticbeanstalk-configuration-template-lifecycle-and-validation#task#5): $.CreateConfigurationTemplateResponse.CreateConfigurationTemplateResult.TemplateUrl missing — expected the S3 template URL (this run: 'https://s3-us-west-2.amazonaws.com/elasticbeanstalk-us-west-2-000000000000/resources/templates/<app>/<template>'), got None. Region, account id, app and template name are the variable parts.
Cause 2 (same fixture #task#8): $.UpdateConfigurationTemplateResponse.UpdateConfigurationTemplateResult.TemplateUrl missing, got None.
Cause 3 (same fixture #task#6, cluster also covers #task#9): $...ConfigurationSettings.member.OptionSettingDetails missing (expected a member list of {Namespace, OptionName, OptionSource{Identifier, Origin}} entries, e.g. Namespace 'aws:elasticbeanstalk:container:python' / OptionName 'NumThreads' / OptionSource {'Identifier':'Service','Origin':'Service'}), got None; ConfigurationSettings.member.EnvironmentLinkPoints missing (expected '', got None); ConfigurationSettings.member.Tier diverges; ~100 OptionSettings.member[...] positions diverge.
Cause 4 (elasticbeanstalk-environment-create-option-setting-reflected#task#5): same three members missing, and the recorded OptionSettingDetails includes {'OptionSource': {'Identifier': 'API', 'Origin': 'API'}, 'Namespace': 'aws:elasticbeanstalk:application:environment', 'OptionName': 'SPRING_PROFILES_ACTIVE'} for the option the CreateEnvironment request set; the emulator's OptionSettings member Namespace/OptionName/ResourceName/Value diverge across most positions.
Cause 5 (elasticbeanstalk-configuration-template-lifecycle-and-validation#task#7): $.DescribeConfigurationOptionsResponse.DescribeConfigurationOptionsResult.Tier missing (expected {'Name':'WebServer','Type':'Standard','Version':'1.0'}, got None); Options.member[0].MaxValue expected '10000' got '20'; Options.member[0].UserDefined expected 'false' got 'true'; Options.member[0].Description missing (expected 'Maximum number of instances you want running. Must be a positive integer that is less than 10000.', got None); identical divergence on Options.member[1] with 'Minimum number of instances you want running. Must be a positive integer no greater than MaxSize.'
Cause 6 (elasticbeanstalk-platform-catalog-and-account-quotas#task#1): $.DescribeAccountAttributesResponse.DescribeAccountAttributesResult.ResourceQuotas.HealthGroupQuota missing (expected {'Maximum': <shaped-not-asserted>}, got None).
Cause 7 (elasticbeanstalk-environment-create-option-setting-reflected#task#4): $.CreateEnvironmentResponse.CreateEnvironmentResult.EnvironmentId value_mismatch — the fixture's shaped assertion ('<shaped-not-asserted>', i.e. AWS's 'e-' + 10-character lowercase alphanumeric form) did not match the emulator's 'e-321524e501824'.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Causes 1-3, 5: eb.create_application(ApplicationName=A); eb.create_configuration_template(ApplicationName=A, TemplateName=T, SolutionStackName=<a Python stack>) -> inspect TemplateUrl (cause 1); eb.describe_configuration_settings(ApplicationName=A, TemplateName=T) -> inspect OptionSettingDetails / EnvironmentLinkPoints / Tier and the resolved OptionSettings (cause 3); eb.describe_configuration_options(ApplicationName=A, TemplateName=T, Options=[{'Namespace':'aws:autoscaling:asg','OptionName':'MaxSize'},{'Namespace':'aws:autoscaling:asg','OptionName':'MinSize'}]) -> inspect Tier, and each option's Description, MaxValue and UserDefined (cause 5); eb.update_configuration_template(ApplicationName=A, TemplateName=T, Description='x') -> inspect TemplateUrl (cause 2).
3. Cause 4/7: eb.create_environment(ApplicationName=A, EnvironmentName=E, SolutionStackName=<same stack>, OptionSettings=[{'Namespace':'aws:elasticbeanstalk:application:environment','OptionName':'SPRING_PROFILES_ACTIVE','Value':'prod'}]) -> check the EnvironmentId shape ('e-' + 10 lowercase alphanumerics), then eb.describe_configuration_settings(ApplicationName=A, EnvironmentName=E) -> look for the OptionSettingDetails entry with OptionSource Origin 'API' for that option.
4. Cause 6: eb.describe_account_attributes() -> inspect ResourceQuotas for HealthGroupQuota.
5. Re-replay elasticbeanstalk-configuration-template-lifecycle-and-validation#task#5, #task#6, #task#7, #task#8; elasticbeanstalk-environment-create-option-setting-reflected#task#4, #task#5; elasticbeanstalk-platform-catalog-and-account-quotas#task#1.

## Resolution

All seven causes are implemented: the template writes report the stored
template's Amazon S3 location derived from the region, the account's storage
bucket and the request's two names; a read of a configuration set reports one
`OptionSettingDetails` entry per resolved setting with the source its value came
from (`API` for a setting the configuration holds, `Service` for a platform
default), the (empty) `EnvironmentLinkPoints` element and the `Tier` the
configuration is for; every published option carries the option reference's
description, the bound that reference states and `UserDefined` false;
`ResourceQuotas` carries `HealthGroupQuota`; and an `EnvironmentId` is minted in
AWS's `e-` + 10-character form.

Two things the replay still reports, left here so the gap is not lost:

* the two `TemplateUrl` keys still report a value difference against the
  recording, whose string the replay withholds. The emulator answers exactly the
  URL this report's Evidence quotes (host form, `elasticbeanstalk-<region>-<account>`
  bucket, `/resources/templates/<app>/<template>`), and the ARN keys of the same
  fixture pass, so the region and account parts are not the difference; what else
  differs cannot be identified from the withheld value.
* the `ConfigurationSettings.member.OptionSettings.member[*]` keys (part of
  causes 3 and 4) stay open, and the attribution above diverges with them on the
  recorded instance: the resolution RULE is implemented, but this emulator
  publishes its own per-platform option catalog (~70 options, ordered by
  namespace then option name) where the recording's platform version publishes
  ~100 in its own order, so the two lists cannot line up position for position.
  Closing that needs AWS's published per-platform-version catalog as data.
