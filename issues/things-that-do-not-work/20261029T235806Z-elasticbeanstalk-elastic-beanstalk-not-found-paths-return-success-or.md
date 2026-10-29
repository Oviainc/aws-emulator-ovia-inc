Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-configuration-template-lifecycle-and-validation
Issue-Step: teardown#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-configuration-template-lifecycle-and-validation elasticbeanstalk-environment-mutation-operations-not-found-errors
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|4xx->2xx|$.ErrorResponse
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|4xx->2xx|status
Issue-Key: elasticbeanstalk|UpdateEnvironment|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticbeanstalk|DeleteEnvironmentConfiguration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# Elastic Beanstalk not-found paths return success or the wrong resource's message (3 causes)

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationSettings

## Description
THREE distinct root causes in the elasticbeanstalk emulator, all in the same service, all on the not-found path of a control-plane read/mutation. Cause 1 returns a 200 where AWS returns a 400; causes 2 and 3 return the right status and code but name the wrong resource or use a non-AWS template. The open behavioral issue 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-refusal-messages-use-non-aws-templates.md covers OTHER rejection paths (it establishes that AssociateEnvironmentOperationsRole uses the "No Environment found for EnvironmentName = '<name>'." template) and does not carry these fixture steps — the replay reported no owner for any of them. THREE causes must be closed before submitting.

Cause 1 of 3: a DELETED configuration template is still readable — DescribeConfigurationSettings answers 200 instead of refusing.
AWS: Smithy DescribeConfigurationSettings(ApplicationName required, TemplateName | EnvironmentName) -> ConfigurationSettingsDescriptions; the model declares only TooManyBucketsException, so the unknown-template rule is service-side validation. Recorded behavior: after the template is deleted, the read is refused with HTTP 400 and an ErrorResponse carrying Error.Type 'Sender', Error.Code 'InvalidParameterValue' and the message template "No Configuration Template named '<ApplicationName>/<TemplateName>' found." — the fixed text is "No Configuration Template named '" ... "' found." and the variable part is the requested application and template names joined by '/'.
Emulator: HTTP 200 with a configuration-settings body and no ErrorResponse at all, i.e. the deleted template is still served from state.
Why: this is the strongest divergence in the set — a client cannot observe its own delete. Delete-then-verify teardown loops conclude the resource still exists and either loop forever or report a failed deletion, and a stale template keeps being usable as a SourceConfiguration for new environments that AWS would refuse to create.
Fix: the template lookup in the DescribeConfigurationSettings handler must treat a deleted template as absent and raise the InvalidParameterValue refusal — services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_configuration_settings.rs together with whatever DeleteConfigurationTemplate leaves behind in crate::state (note the open stub issue 20261029T180136Z-elasticbeanstalk-deleteconfigurationtemplate-stub.md for that operation).
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#teardown#13.

Cause 2 of 3: UpdateEnvironment's environment-not-found message uses the generic template instead of the account-scoped one.
AWS: Smithy UpdateEnvironment(ApplicationName, EnvironmentId, EnvironmentName, ...) -> EnvironmentDescription, modelled errors InsufficientPrivilegesException (403) / TooManyBucketsException (400); the unknown-environment rule is service-side. Recorded behavior: HTTP 400, Error.Code 'InvalidParameterValue', message template "Environment not found with Name: <EnvironmentName> in account '<accountId>'" — the fixed text is "Environment not found with Name: " ... " in account '" ... "'" (no trailing period, and the name is NOT quoted), with the requested EnvironmentName and the caller's account id substituted.
Emulator: same status and code, but the message is "No Environment found for EnvironmentName = '<EnvironmentName>'." — the generic environment-not-found template (which AWS does use on other operations, per the open issue named above), so UpdateEnvironment's own wording and the account component are lost.
Why: operators and clients that match AWS's UpdateEnvironment wording — or parse the account id out of it to diagnose a cross-account mistake — do not recognise the refusal, and a transcript diff against real AWS fails on every update of a missing environment.
Fix: the environment-lookup error construction in the UpdateEnvironment handler — src/ops/update_environment.rs; it must use the account-scoped template rather than the crate's shared generic environment-not-found builder.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#2.

Cause 3 of 3: DeleteEnvironmentConfiguration validates the environment before the application, so it reports the wrong missing resource.
AWS: Smithy DeleteEnvironmentConfiguration(ApplicationName required, EnvironmentName required) -> Unit, no modelled operation errors; the not-found rules are service-side. Recorded behavior: when BOTH the application and the environment are unknown, the APPLICATION is validated first and the call is refused with HTTP 400, Error.Code 'InvalidParameterValue' and the message template "No Application named '<ApplicationName>' found." — fixed text around the requested application name.
Emulator: HTTP 400 / InvalidParameterValue but with "No Environment found for EnvironmentName = '<EnvironmentName>'." — it skips the application check and diagnoses the environment, so the message names a different resource than AWS names.
Why: the caller is told to fix the environment name when the real problem is the application name, which sends automated remediation after the wrong resource; the validation ORDER (application before environment) is the behavioural requirement here, not just the wording.
Fix: the input-validation order and error construction in the DeleteEnvironmentConfiguration handler — src/ops/delete_environment_configuration.rs: resolve ApplicationName first and raise the application-not-found refusal before looking at EnvironmentName.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#10.

## Evidence
Cause 1 (elasticbeanstalk-configuration-template-lifecycle-and-validation#teardown#13): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': "No Configuration Template named '<app>/<template>' found."}, 'RequestId': ...}, got None. In this run the substituted names were 'sim2real-probe-elasticbeanstalk-app-3949ae7a/sim2real-probe-elasticbeanstalk-tmpl-3949ae7a' and the RequestId was per-run (not a requirement).
Cause 2 (elasticbeanstalk-environment-mutation-operations-not-found-errors#task#2): status expected 400 / actual 400 (code matches); $.ErrorResponse.Error.Message value_mismatch — expected "Environment not found with Name: <EnvironmentName> in account '<accountId>'" (this run: "Environment not found with Name: sim2real-probe-missing-env-b0211e0a in account '000000000000'"), got "No Environment found for EnvironmentName = 'sim2real-probe-missing-env-b0211e0a'." The environment name and account id are the variable parts; the surrounding sentence is fixed.
Cause 3 (elasticbeanstalk-environment-mutation-operations-not-found-errors#task#10): status expected 400 / actual 400; $.ErrorResponse.Error.Message value_mismatch — expected "No Application named '<ApplicationName>' found." (this run: 'sim2real-probe-elasticbeanstalk-missing-app-beb4629f'), got "No Environment found for EnvironmentName = 'sim2real-probe-missing-env-b0211e0a'." — the emulator named the environment where AWS names the application.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.create_application(ApplicationName=A); eb.create_configuration_template(ApplicationName=A, TemplateName=T, SolutionStackName=<any stack>); eb.delete_configuration_template(ApplicationName=A, TemplateName=T); then eb.describe_configuration_settings(ApplicationName=A, TemplateName=T) -> expect HTTP 400 InvalidParameterValue "No Configuration Template named 'A/T' found.", observe HTTP 200 with settings.
3. Cause 2: eb.update_environment(EnvironmentName=<a name that does not exist>, Description='x') -> compare Error.Message against "Environment not found with Name: <name> in account '<accountId>'".
4. Cause 3: eb.delete_environment_configuration(ApplicationName=<an application that does not exist>, EnvironmentName=<an environment that does not exist>) -> expect "No Application named '<ApplicationName>' found.", observe the environment-not-found message.
5. Re-replay elasticbeanstalk-configuration-template-lifecycle-and-validation#teardown#13, elasticbeanstalk-environment-mutation-operations-not-found-errors#task#2 and #task#10.
