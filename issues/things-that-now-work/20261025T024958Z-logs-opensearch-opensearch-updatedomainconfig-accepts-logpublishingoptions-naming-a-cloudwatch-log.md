Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups logs__opensearch.osdocstat-describe-domains-batch-matches-describe-domain
Issue-Key: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups|run#18|AssertionError|assert rejected is not None, (
Issue-Key: logs__opensearch.osdocstat-describe-domains-batch-matches-describe-domain|run#18|AssertionError|assert "LogPublishingOptions" not in by_name[quiet], (
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# opensearch UpdateDomainConfig accepts LogPublishingOptions naming a CloudWatch log group that does not exist (200 instead of 400 ValidationException)

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainConfig

## Description
One net-new root cause in the opensearch service (plus one additional affected check grouped for replay only — see the end of this description).

Cause 1 of 1: UpdateDomainConfig enables a LogPublishingOption whose CloudWatchLogsLogGroupArn names a log group that does not exist, instead of refusing the change.

AWS: UpdateDomainConfig validates the destination of an enabled log-publishing option before applying it. A request whose LogPublishingOptions entry carries Enabled=true and a CloudWatchLogsLogGroupArn in the caller's own account and Region for a group nobody created is refused synchronously with HTTP 400 ValidationException, and the domain's publishing options are left untouched. Smithy: opensearch#UpdateDomainConfig models ValidationException (httpError 400) and ResourceNotFoundException (409); its input member LogPublishingOptions targets the LogPublishingOptions map of LogPublishingOption {CloudWatchLogsLogGroupArn, Enabled}, and the operation's success shape is a 200 DomainConfig — there is no success response that means "accepted but the destination is bogus". The existence of the named group is AWS-documented behavior rather than a modeled constraint, so the requirement here is the 400 ValidationException and the absence of any stored enabled option; the recorded refusal message is an example only.

Emulator: the call is accepted. The replayed check asserts the refusal and reports 'expected UpdateDomainConfig naming the nonexistent log group arn:aws:logs:<region>:<account>:log-group:sim2real-behav-osidx-missing-<id> to be REJECTED, but it was accepted' — i.e. the emulator answered 200 and began applying the change. Only the shape of the ARN is checked, never whether the CloudWatch Logs group behind it exists (and, by extension, never whether a resource policy lets es.amazonaws.com write to it).

Why: a client that wires domain observability gets a silent no-op. The domain reports LogPublishingOptions pointing at a destination that cannot receive anything, so "logs are enabled" is true in the emulated estate and false on AWS, and the negative test that is supposed to catch a typo'd or not-yet-created log group passes against the emulator and fails on AWS. Nothing downstream — slow-log delivery, log-group-scoped alarms — can be relied upon.

Fix: the UpdateDomainConfig handler's LogPublishingOptions validation in the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_domain_config.rs): resolve each enabled option's CloudWatchLogsLogGroupArn against CloudWatch Logs state and return ValidationException when the group does not exist, before any part of the change is stored. Related but distinct: the open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md covers the Enabled=true-with-NO-ARN case on CreateDomain (its Cause 4); the gap here is the existence of the group named by a well-formed ARN on the update path, which that issue's causes do not include.

Traces: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups.

Additional affected check (NO separate cause, grouped for replay): logs__opensearch.osdocstat-describe-domains-batch-matches-describe-domain fails with 'expected sim2real-osds-<id> to carry no LogPublishingOptions in the batch read, got {}'. That is the SAME root cause already filed as Cause 5 of the open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md (DomainStatus emits an unset optional map as {} instead of omitting the member): DescribeDomains and DescribeDomain share one serializer — Domain::to_status() in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs emits log_publishing_options: Some(...) unconditionally — so closing that cause closes this check too. It is listed here only so a fixer re-replays it rather than leaving it unrun; do not count it as a second cause of this issue.

## Evidence
Cause 1: AssertionError: expected UpdateDomainConfig naming the nonexistent log group arn:aws:logs:us-west-2:000000000000:log-group:sim2real-behav-osidx-missing-7976a4a6963b to be REJECTED, but it was accepted (expected: HTTP 400 ValidationException, no option stored; got: the call succeeded). The check verified immediately beforehand, via logs.describe_log_groups(logGroupNamePrefix=<name>) == [], that no such log group existed.

Grouped-for-replay check (cause already filed elsewhere): AssertionError: expected sim2real-osds-a9fec67e93d5 to carry no LogPublishingOptions in the batch read, got {} — DescribeDomains returns the member as an empty map for a domain created with no log publishing. Source confirmation in the checkout: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs, Domain::to_status(), log_publishing_options: Some(self.log_publishing_options.iter()...collect()).

## Steps to Reproduce
Cause 1:
1. logs.create_log_group(logGroupName='g-missing-probe') is NOT called — pick a log-group name that does not exist and confirm with logs.describe_log_groups(logGroupNamePrefix=<name>) -> logGroups == [].
2. opensearch.create_domain(DomainName=<d>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}) and wait for Processing=False.
3. opensearch.update_domain_config(DomainName=<d>, LogPublishingOptions={'INDEX_SLOW_LOGS': {'CloudWatchLogsLogGroupArn': 'arn:aws:logs:<region>:<account>:log-group:<name-from-step-1>', 'Enabled': True}}).
   Observed: HTTP 200, the option is applied. Expected: HTTP 400 ValidationException and describe_domain showing no enabled publishing option and no trace of that ARN.

Grouped-for-replay check: create two domains, one with LogPublishingOptions and one with none, then opensearch.describe_domains(DomainNames=[both]) -> the second DomainStatus contains 'LogPublishingOptions': {} (expected: the member absent).

Replay the check ids listed in trace_ids.
