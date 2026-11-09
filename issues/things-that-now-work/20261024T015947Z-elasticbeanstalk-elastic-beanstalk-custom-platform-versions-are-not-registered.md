Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.create-platform-version-too-many-platforms-rejected elasticbeanstalk.custom-platform-version-tags-round-trip elasticbeanstalk.platform-version-custom-tags-round-trip elasticbeanstalk.create-platform-version-too-many-fails elasticbeanstalk.create-platform-version-insufficient-privileges-blocked elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence elasticbeanstalk.delete-environment-configuration-removes-draft-configuration elasticbeanstalk.platform-branch-lifecycle-state-preserved elasticbeanstalk.foreign-resource-arn-for-tagging-rejected elasticbeanstalk.update-tags-for-resource-same-key-add-and-remove-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.create-platform-version-too-many-platforms-rejected|run#16|AssertionError|assert at_quota == quota, (
Issue-Key: elastic-beanstalk|ListTagsForResource|ResourceNotFoundException
Issue-Key: elasticbeanstalk.create-platform-version-too-many-fails|run#20|AssertionError|assert message is None, (
Issue-Key: elasticbeanstalk.create-platform-version-insufficient-privileges-blocked|run#10|AssertionError|raise AssertionError("expected CreatePlatformVersion to reject the denied caller, but it succeeded")
Issue-Key: elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence|run#24|AssertionError|assert dns_a["Available"] is True, (
Issue-Key: elasticbeanstalk.delete-environment-configuration-removes-draft-configuration|run#18|AssertionError|assert _option_value(settings[0], _OPTION_NS, _OPTION_NAME) == template_value, (
Issue-Key: elasticbeanstalk.platform-branch-lifecycle-state-preserved|run#12|AssertionError|assert match["LifecycleState"] == expected_state, (
Issue-Key: elasticbeanstalk.foreign-resource-arn-for-tagging-rejected|run#8|AssertionError|assert write_message == expected_message, (
Issue-Key: elasticbeanstalk.update-tags-for-resource-same-key-add-and-remove-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Elastic Beanstalk custom platform versions are not registered (ListPlatformVersions/ListTagsForResource), CreatePlatformVersion is unguarded, CNAME prefixes are never released, template options are dropped, and two rejection shapes diverge

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePlatformVersion

## Description
Seven distinct root causes in the elasticbeanstalk emulator, all in the same service, all grounded in failing behavioral checks of this suite. Two open issues cover adjacent ground but not these checks: 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md (owns the '-denied' authorization variants, not the '-blocked' one below) and 20261022T233132Z-elasticbeanstalk-elastic-beanstalk-listplatformbranches-serves-a-placeholder-branch-catalog.md (cause 6 is the cross-API consequence of it). SEVEN causes must be closed before submitting.

Cause 1 of 7: a platform version created by CreatePlatformVersion is not registered as a resource — it is invisible to ListPlatformVersions and unknown to ListTagsForResource.
AWS: Smithy: CreatePlatformVersion -> CreatePlatformVersionResult{PlatformSummary{PlatformArn,...}}; ListPlatformVersions(Filters) -> PlatformSummaryList (filters include PlatformOwner and PlatformName); ListTagsForResource(ResourceArn) -> ResourceTagsDescriptionMessage{ResourceArn, ResourceTags}, with ResourceNotFoundException (400) reserved for ARNs that are not an existing EB resource. Recorded behavior: the ARN returned by CreatePlatformVersion is immediately listable under PlatformOwner='Self' and its CreatePlatformVersion-time Tags round-trip through ListTagsForResource.
Emulator: CreatePlatformVersion returns 200 with a PlatformSummary (PlatformStatus='Creating') and an ARN, but ListPlatformVersions filtered on PlatformOwner='Self' counts 0, and ListTagsForResource on the very ARN just returned raises ResourceNotFoundException: "Resource not found for ARN 'arn:aws:elasticbeanstalk:us-west-2:000000000000:platform/sim2real-eb-plat-cfe134ae02fc/1.0.0'."
Why: a client that creates a custom platform can never find it, poll it, tag it or delete it; the create call reports success while leaving nothing behind.
Fix: the CreatePlatformVersion handler must persist the platform into the service's platform/tag state that ListPlatformVersions and ListTagsForResource read (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/ plus the service state module).
Traces: elasticbeanstalk.create-platform-version-too-many-platforms-rejected, elasticbeanstalk.custom-platform-version-tags-round-trip, elasticbeanstalk.platform-version-custom-tags-round-trip.

Cause 2 of 7: the over-quota CreatePlatformVersion rejection carries a Message element that AWS's does not.
AWS: Smithy: CreatePlatformVersion error TooManyPlatformsException, client error, HTTP 400, awsQueryError code 'TooManyPlatformsException'. Recorded behavior: the Error element carries NO Message at all.
Emulator: same code and status (TooManyPlatformsException / 400) but Error.Message is populated, e.g. 'The number of custom platform versions in this account has reached the limit of 50.' The divergence is the presence of the Message member, not its wording.
Why: clients and SDK error-shape assertions that key on the absence of Message (or surface it to users) see a response shape AWS never emits.
Fix: the CreatePlatformVersion quota-rejection serialization — emit TooManyPlatformsException with no message member.
Traces: elasticbeanstalk.create-platform-version-too-many-fails.

Cause 3 of 7: CreatePlatformVersion is not authorization-gated.
AWS: Smithy: CreatePlatformVersion declares InsufficientPrivilegesException, client error, HTTP 403. Recorded behavior: a caller whose IAM policy denies elasticbeanstalk:CreatePlatformVersion is rejected (403 / AccessDenied-class error) and no platform is created.
Emulator: the restricted caller's call SUCCEEDS with HTTP 200 and a PlatformSummary (PlatformStatus 'Creating', ARN arn:aws:elasticbeanstalk:us-west-2:000000000000:platform/<name>/1.0.0).
Why: an overly permissive control plane: policy regressions are invisible and least-privilege tests pass against the emulator while failing on AWS.
Fix: the authorization gate in the CreatePlatformVersion handler (the same gate the open control-plane-guards issue describes for the sibling '-denied' checks; this '-blocked' check is not covered there).
Traces: elasticbeanstalk.create-platform-version-insufficient-privileges-blocked.

Cause 4 of 7: a CNAME prefix stays reserved after SwapEnvironmentCNAMEs releases it.
AWS: Smithy: CheckDNSAvailability(CNAMEPrefix) -> {Available: boolean, FullyQualifiedCNAME}. Recorded behavior: once a swap moves a prefix off an environment, that prefix is no longer in use and CheckDNSAvailability reports Available=true for it.
Emulator: after the swap, CheckDNSAvailability still reports {'Available': False, 'FullyQualifiedCNAME': '<prefix>.us-west-2.elasticbeanstalk.com'} for the released prefix.
Why: a client cannot reuse a prefix it legitimately freed; blue/green CNAME rotation reports the name permanently taken.
Fix: the CNAME-reservation state updated by SwapEnvironmentCNAMEs and read by CheckDNSAvailability.
Traces: elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence.

Cause 5 of 7: an environment created from a configuration template does not inherit the template's OptionSettings.
AWS: Smithy: DescribeConfigurationSettings -> ConfigurationSettingsDescriptions of ConfigurationSettingsDescription{DeploymentStatus, OptionSettings: ConfigurationOptionSettingsList}. Recorded behavior: for an environment created with TemplateName, the single deployed configuration set (DeploymentStatus='deployed') carries the template's option values.
Emulator: the deployed set is present with DeploymentStatus='deployed', but the template's custom option is absent — AssertionError "expected the template's option value 'template-value', got None".
Why: configuration templates are silently inert: an environment launched from a saved configuration runs with different settings than the template declares.
Fix: CreateEnvironment's template resolution (merge the referenced configuration template's OptionSettings into the environment's configuration set) and the DescribeConfigurationSettings read path.
Traces: elasticbeanstalk.delete-environment-configuration-removes-draft-configuration.

Cause 6 of 7: the branch LifecycleState reported by ListPlatformBranches and by ListPlatformVersions disagree in case.
AWS: Smithy: PlatformBranchSummary.LifecycleState (PlatformBranchLifecycleState) and PlatformSummary.PlatformBranchLifecycleState describe the same branch metadata; recorded behavior: the two APIs return the identical string for a given branch (no case normalization between them).
Emulator: branch '.NET 8 running on 64bit Amazon Linux 2023' reports LifecycleState='supported' via ListPlatformBranches but 'Supported' via ListPlatformVersions.
Why: a client joining platform versions to their branch metadata (or filtering on the lifecycle state) gets no match for strings AWS keeps consistent.
Fix: make the platform catalog one source of truth for branch lifecycle state across the ListPlatformBranches and ListPlatformVersions serializers; see the open issue about the placeholder branch catalog with lowercase lifecycle states.
Traces: elasticbeanstalk.platform-branch-lifecycle-state-preserved.

Cause 7 of 7: two UpdateTagsForResource rejection messages do not follow AWS's rule.
AWS: Smithy: UpdateTagsForResource(ResourceArn, TagsToAdd, TagsToRemove) with ResourceNotFoundException (400). Recorded behavior, two rules: (a) a non-Elastic-Beanstalk resource ARN is refused with the request-derived message "The name '<arn>' isn't a valid ARN of an Elastic Beanstalk resource."; (b) the same tag key in both TagsToAdd and TagsToRemove is refused with the fixed message "A tag with the same key can't be added and removed in a single operation." (no request-derived part).
Emulator: same error code and status, but different texts: "Resource ARN '<arn>' is not an Elastic Beanstalk resource ARN." and "The tag key 'sim2real-contested' cannot be specified in both TagsToAdd and TagsToRemove."
Why: clients and tests that surface or match on these two deterministic messages diverge; (b) also leaks a request value into a message AWS keeps constant.
Fix: the rejection messages in the UpdateTagsForResource handler's validation.
Traces: elasticbeanstalk.foreign-resource-arn-for-tagging-rejected, elasticbeanstalk.update-tags-for-resource-same-key-add-and-remove-rejected.

## Evidence
Cause 1: expected the ARN returned by CreatePlatformVersion to be listable and taggable; got AssertionError 'expected the account to sit at its CustomPlatformQuota of 50 after creating 50 platform version(s), ListPlatformVersions counts 0' and ResourceNotFoundException from ListTagsForResource: "Resource not found for ARN 'arn:aws:elasticbeanstalk:us-west-2:000000000000:platform/sim2real-eb-plat-cfe134ae02fc/1.0.0'." (also .../sim2real-eb-plat-596d1aa99929/1.0.0).
Cause 2: expected TooManyPlatformsException with no Message member; got code TooManyPlatformsException plus Message 'The number of custom platform versions in this account has reached the limit of 50.'
Cause 3: expected a 403 InsufficientPrivileges-class rejection for the denied caller; got HTTP 200 with {'PlatformSummary': {'PlatformArn': 'arn:aws:elasticbeanstalk:us-west-2:000000000000:platform/sim2real-beh-platform-154c9bc2ba91/1.0.0', 'PlatformStatus': 'Creating', ...}} — AssertionError 'expected CreatePlatformVersion to reject the denied caller, but it succeeded'.
Cause 4: expected Available=true for the released prefix; got {'Available': False, 'FullyQualifiedCNAME': 'sim2real-swp-a-174dd1cdc974-7b3b115902104.us-west-2.elasticbeanstalk.com'}.
Cause 5: expected the deployed configuration set to carry the template option value 'template-value'; got None.
Cause 6: expected the same LifecycleState from both APIs; got 'supported' (ListPlatformBranches) vs 'Supported' (ListPlatformVersions).
Cause 7: expected "The name '<arn>' isn't a valid ARN of an Elastic Beanstalk resource." / "A tag with the same key can't be added and removed in a single operation."; got "Resource ARN '<arn>' is not an Elastic Beanstalk resource ARN." / "The tag key 'sim2real-contested' cannot be specified in both TagsToAdd and TagsToRemove." (status and error code matched in both).

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.create_storage_location, s3.put_object a bundle, eb.create_platform_version(PlatformName, PlatformVersion='1.0.0', PlatformDefinitionBundle=..., Tags=[...]); then eb.list_platform_versions(Filters=[{'Type':'PlatformOwner','Operator':'=','Values':['Self']}]) -> empty, and eb.list_tags_for_resource(ResourceArn=<returned PlatformArn>) -> ResourceNotFoundException.
3. Cause 2: repeat CreatePlatformVersion until the quota from DescribeAccountAttributes.ResourceQuotas.CustomPlatformQuota is exhausted, then call it once more -> TooManyPlatformsException WITH a Message element.
4. Cause 3: assume an IAM role denying elasticbeanstalk:CreatePlatformVersion and call eb.create_platform_version -> 200 instead of 403.
5. Cause 4: create two environments with CNAME prefixes A and B, wait Ready, eb.swap_environment_cnames, then eb.check_dns_availability(CNAMEPrefix=A) -> Available False.
6. Cause 5: eb.create_configuration_template with a custom OptionSettings entry, eb.create_environment(TemplateName=...), wait Ready, eb.describe_configuration_settings -> the custom option is missing.
7. Cause 6: eb.list_platform_versions(MaxRecords=20), take PlatformBranchName/PlatformBranchLifecycleState, then eb.list_platform_branches(Filters=[{'Attribute':'BranchName','Operator':'=','Values':[branch]}]) -> LifecycleState differs in case.
8. Cause 7: eb.update_tags_for_resource with a non-EB ARN, and with the same tag key in TagsToAdd and TagsToRemove -> message texts differ from AWS's.
9. Re-replay the checks listed in trace_ids.
