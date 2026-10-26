Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.albos-master-user-password-without-a-special-character-rejected opensearch.off-peak-window-and-software-update-options opensearch.overly-permissive-storage-tier-dependency-rules opensearch.osdocstat-enabled-log-option-without-group-arn-rejected opensearch.osdocstat-domain-without-log-publishing-omits-the-member opensearch.service-software-ops-on-missing-domain opensearch.update-vpc-endpoint-invalid-subnet-rejected opensearch.update-vpc-endpoint-out-of-range-config-rejected opensearch.data-source-lifecycle opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation opensearch.visibility-timeout-ttl-service-software-rollback-window-accepted
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.albos-master-user-password-without-a-special-character-rejected|_error#7|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: opensearch.off-peak-window-and-software-update-options|_expect_rejection#5|AssertionError|raise AssertionError(
Issue-Key: opensearch.overly-permissive-storage-tier-dependency-rules|_expect_rejection#2|AssertionError|raise AssertionError(
Issue-Key: opensearch.osdocstat-enabled-log-option-without-group-arn-rejected|run#6|AssertionError|assert refused is not None, (
Issue-Key: opensearch.osdocstat-domain-without-log-publishing-omits-the-member|run#9|AssertionError|assert "LogPublishingOptions" not in status, (
Issue-Key: opensearch.service-software-ops-on-missing-domain|run#4|AssertionError|assert code == 'ResourceNotFoundException', f'StartServiceSoftwareUpdate: got {code}'
Issue-Key: opensearch|CreateTags|InvalidID
Issue-Key: service-quotas|GetServiceQuota|NoSuchResourceException
Issue-Key: opensearch.data-source-lifecycle|_add_data_source_with_retry#8|Timeout|time.sleep(interval_s)
Issue-Key: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation|_wait_master_auth#8|Timeout|time.sleep(10.0)
Issue-Key: opensearch.visibility-timeout-ttl-service-software-rollback-window-accepted|_wait_service_software_settled#6|Timeout|time.sleep(interval_s)
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# OpenSearch CreateDomain skips documented validation, DescribeDomain fabricates empty members, and missing-domain/service-software calls answer NotImplemented

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Seven distinct root causes in the opensearch service. All seven must be closed before this issue is resubmitted.

Cause 1 of 7: CreateDomain accepts a fine-grained-access-control master password that does not meet the password requirements.
AWS: CreateDomain with AdvancedSecurityOptions.MasterUserOptions refuses a master password that fails the documented complexity requirements (minimum 8 characters and at least one uppercase letter, one lowercase letter, one digit and one special character) with HTTP 400 ValidationException, and creates no domain. Smithy: opensearch#MasterUserOptions$MasterUserPassword has @length min 8 max 128 and pattern '.*' only — the complexity rule is AWS-documented rather than modeled, so the requirement here is the 400 ValidationException and the absence of a created domain; the recorded refusal message is an example.
Emulator: the call returns HTTP 200 with a full DomainStatus (DomainId/ARN/Endpoint) for a password with no special character — 'expected a rejection, but it succeeded'.
Why: a client's negative test for weak credentials passes, and the emulated estate holds an FGAC domain whose master credential real AWS would never have accepted — the configuration is not portable to AWS.
Fix: CreateDomain's AdvancedSecurityOptions/MasterUserOptions validation in the OpenSearch service crate.
Traces: opensearch.albos-master-user-password-without-a-special-character-rejected.

Cause 2 of 7: CreateDomain accepts OffPeakWindowOptions with Enabled=false.
AWS: the off-peak window cannot be disabled at create time. Smithy: opensearch#OffPeakWindowOptions$Enabled documents 'This option is only available when modifying a domain created prior to February 16, 2023, not when creating a new domain. All domains created after this date have the off-peak window enabled by default. You can't disable the off-peak window after it's enabled for a domain.' So CreateDomain with Enabled=false is refused with HTTP 400 ValidationException (modeled on CreateDomain) and creates no domain.
Emulator: CreateDomain succeeds and returns a DomainId — 'OffPeakWindowOptions with Enabled=False: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-f76432358f'.
Why: the emulator is more permissive than AWS on a documented create-time restriction, so a template or SDK call that the emulator accepts fails on real AWS, and the maintenance-scheduling invariant (every new domain has an off-peak window) does not hold in the emulated estate.
Fix: CreateDomain's OffPeakWindowOptions validation in the OpenSearch service crate.
Traces: opensearch.off-peak-window-and-software-update-options.

Cause 3 of 7: CreateDomain accepts ColdStorageOptions.Enabled=true without UltraWarm enabled.
AWS: cold storage depends on warm storage. Smithy: opensearch#ColdStorageOptions$Enabled documents 'Whether to enable or disable cold storage on the domain. You must enable UltraWarm storage to enable cold storage.' A ClusterConfig with ColdStorageOptions={'Enabled': true} but WarmEnabled unset/false is therefore refused with HTTP 400 ValidationException and creates no domain.
Emulator: CreateDomain succeeds — 'ColdStorage without UltraWarm: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-323d039fd1'.
Why: an impossible storage tiering is materialized. A client exercising tier-dependency rules sees them as nonexistent, and a configuration validated against the emulator is rejected by AWS.
Fix: CreateDomain's ClusterConfig storage-tier dependency validation (ColdStorageOptions requires WarmEnabled/WarmType/WarmCount) in the OpenSearch service crate.
Traces: opensearch.overly-permissive-storage-tier-dependency-rules.

Cause 4 of 7: CreateDomain accepts a LogPublishingOption with Enabled=true and no CloudWatchLogsLogGroupArn.
AWS: an enabled log-publishing option must name the destination log group, so CreateDomain with LogPublishingOptions={'ES_APPLICATION_LOGS': {'Enabled': true}} and no CloudWatchLogsLogGroupArn is refused with HTTP 400 ValidationException and creates no domain. Smithy: opensearch#LogPublishingOption = {CloudWatchLogsLogGroupArn (length 20..2048), Enabled}; CreateDomain models ValidationException.
Emulator: the request is accepted — 'expected CreateDomain with ES_APPLICATION_LOGS Enabled=true and no log group ARN to be REJECTED, but it was accepted'.
Why: the domain claims to publish logs to nowhere. A client that wires up observability gets a silent no-op instead of the validation error that tells it the destination is missing.
Fix: CreateDomain/UpdateDomainConfig LogPublishingOptions validation in the OpenSearch service crate.
Traces: opensearch.osdocstat-enabled-log-option-without-group-arn-rejected.

Cause 5 of 7: DescribeDomain reports LogPublishingOptions as an empty map on a domain created without it, instead of omitting the member.
AWS: DomainStatus omits LogPublishingOptions entirely for a domain that was created without any log-publishing option. Smithy: opensearch#DomainStatus$LogPublishingOptions is an optional map member with no default, so an unset map is absent from the response rather than serialized as {}.
Emulator: the member is present as an empty map — 'expected DescribeDomain to OMIT LogPublishingOptions on a domain created without it, got {}'.
Why: presence-vs-absence is how clients distinguish 'never configured' from 'configured and empty'. Code that branches on `'LogPublishingOptions' in status` takes the wrong branch, and config-drift comparisons report a difference that does not exist.
Fix: the DomainStatus serializer in the OpenSearch service crate — do not emit unset optional map/struct members.
Traces: opensearch.osdocstat-domain-without-log-publishing-omits-the-member.

Cause 6 of 7: service-software operations on a missing domain answer NotImplemented instead of ResourceNotFoundException.
AWS: StartServiceSoftwareUpdate against a domain that does not exist is refused with ResourceNotFoundException. Smithy: opensearch#StartServiceSoftwareUpdate models ResourceNotFoundException (httpError 409) and ValidationException (400); NotImplemented is not in its error set and no 501 is part of its contract.
Emulator: the call returns error code 'NotImplemented' — 'StartServiceSoftwareUpdate: got NotImplemented'.
Why: a client cannot distinguish 'this domain does not exist' from 'the emulator has no handler'. Not-found handling, retry logic and error-code assertions all break, and the operation's modeled contract is violated.
Fix: the StartServiceSoftwareUpdate handler in the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/) — resolve the domain and return ResourceNotFoundException when absent.
Traces: opensearch.service-software-ops-on-missing-domain.

Cause 7 of 7: the supporting calls an OpenSearch VPC-endpoint scenario needs are broken — EC2 CreateTags rejects a subnet id EC2 CreateSubnet just minted, and Service Quotas has no VPC quota L-2AFB9258.
AWS: ec2:CreateTags accepts any existing resource id, including a subnet id returned by ec2:CreateSubnet moments earlier (an id of the form subnet-<17 hex chars>), and returns 200; servicequotas:GetServiceQuota(ServiceCode='vpc', QuotaCode='L-2AFB9258') returns 200 with that quota's Value. Recorded behavior only for the specific quota code; the requirement is that an id the emulator itself issued is accepted by its own tagging API, and that a real VPC quota code resolves.
Emulator: ec2.create_tags(Resources=['subnet-00000000000000020'], ...) fails with error code InvalidID and 'The ID 'subnet-00000000000000020' is not valid' even though ec2.create_subnet returned that id; servicequotas.get_service_quota(ServiceCode='vpc', QuotaCode='L-2AFB9258') fails with NoSuchResourceException "no quota with ServiceCode 'vpc' and QuotaCode 'L-2AFB9258' exists". Both abort the UpdateVpcEndpoint negative checks before any opensearch call is graded.
Why: the emulator is internally inconsistent — resources it creates are rejected by its own id validator — so any client that tags what it creates fails, and quota-aware code cannot read a standard VPC quota. The UpdateVpcEndpoint same-VPC validation these checks exist to grade is never reached.
Fix: the EC2 CreateTags resource-id validator (accept the 17-character id form the emulator's own CreateSubnet emits) and the Service Quotas quota catalog for ServiceCode 'vpc'. Note: these checks are reported under the opensearch service token, but the defective components are in the EC2 and Service Quotas crates.
Traces: opensearch.update-vpc-endpoint-invalid-subnet-rejected, opensearch.update-vpc-endpoint-out-of-range-config-rejected.

Additional affected checks (no separate cause, grouped for replay): opensearch.data-source-lifecycle, opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity, opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation and opensearch.visibility-timeout-ttl-service-software-rollback-window-accepted all run to the harness deadline ('Check timed out after 60.0s') because the state they poll for never arrives — the first on the data-source lifecycle, the middle two on data-plane document continuity, the last on a service-software rollback window. Hypothesis: they are downstream of the data-plane and ServiceSoftwareOptions/data-source causes already named (here and in the companion opensearch issue filed in this sweep); they are listed so a fixer re-replays them rather than leaving them unrun.

## Evidence
Cause 1: AssertionError: expected a rejection, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'DomainStatus': {'DomainId': '000000000000/sim2real-ao-98d5917b9817', 'ARN': 'arn:aws:es:us-west-2:000000000000:domain/sim2real-ao-98d5917b9817', ...}}
Cause 2: AssertionError: OffPeakWindowOptions with Enabled=False: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-f76432358f
Cause 3: AssertionError: ColdStorage without UltraWarm: CreateDomain should have been rejected but succeeded with DomainId=000000000000/sim2real-behav-os-323d039fd1
Cause 4: AssertionError: expected CreateDomain with ES_APPLICATION_LOGS Enabled=true and no log group ARN to be REJECTED, but it was accepted
Cause 5: AssertionError: expected DescribeDomain to OMIT LogPublishingOptions on a domain created without it, got {}
Cause 6: AssertionError: StartServiceSoftwareUpdate: got NotImplemented (expected ResourceNotFoundException)
Cause 7: botocore.exceptions.ClientError: An error occurred (InvalidID) when calling the CreateTags operation: The ID 'subnet-00000000000000020' is not valid — on an id returned by this emulator's own ec2.create_subnet; and botocore.errorfactory.NoSuchResourceException: An error occurred (NoSuchResourceException) when calling the GetServiceQuota operation: no quota with ServiceCode 'vpc' and QuotaCode 'L-2AFB9258' exists
Grouped timeouts: Error: Check timed out after 60.0s (x4).

## Steps to Reproduce
Cause 1: opensearch.create_domain(..., AdvancedSecurityOptions={'Enabled':True,'InternalUserDatabaseEnabled':True,'MasterUserOptions':{'MasterUserName':'adminuser','MasterUserPassword':'Abcdefgh1234'}}) -> 200 (expected 400 ValidationException).
Cause 2: opensearch.create_domain(..., OffPeakWindowOptions={'Enabled': False, 'OffPeakWindow': {...}}) -> 200 (expected 400 ValidationException).
Cause 3: opensearch.create_domain(..., ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False,'ColdStorageOptions':{'Enabled':True}}) -> 200 (expected 400 ValidationException).
Cause 4: opensearch.create_domain(..., LogPublishingOptions={'ES_APPLICATION_LOGS': {'Enabled': True}}) -> 200 (expected 400 ValidationException).
Cause 5: opensearch.create_domain without LogPublishingOptions, then describe_domain -> DomainStatus contains 'LogPublishingOptions': {} (expected the member absent).
Cause 6: opensearch.start_service_software_update(DomainName='does-not-exist-xyz') -> NotImplemented (expected ResourceNotFoundException).
Cause 7: sid = ec2.create_subnet(VpcId=<vpc>, CidrBlock='10.30.1.0/24')['Subnet']['SubnetId']; ec2.create_tags(Resources=[sid], Tags=[{'Key':'Name','Value':'x'}]) -> InvalidID. servicequotas.get_service_quota(ServiceCode='vpc', QuotaCode='L-2AFB9258') -> NoSuchResourceException.

Replay the check ids listed in trace_ids.
