Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.instance-catalog-enum-semantics opensearch.service-software-rollback-and-domain-upgrade
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.instance-catalog-enum-semantics|run#11|AssertionError|assert code == "ValidationException", f'bogus EngineVersion should be rejected, got {code}'
Issue-Key: opensearch.service-software-rollback-and-domain-upgrade|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: 221ac750993b763951d073216bc44789c013bd0c

# opensearch: DescribeInstanceTypeLimits accepts an engine version that does not exist, and StartServiceSoftwareUpdate answers NotImplemented when no update is available (2 causes)

> _Note: this issue was identified against the code base at commit `221ac750993b763951d073216bc44789c013bd0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypeLimits

## Description
Two distinct net-new root causes in the opensearch service. BOTH must be closed before this issue is resubmitted.

Cause 1 of 2: DescribeInstanceTypeLimits does not validate EngineVersion against the engine catalog — an engine version that does not exist is answered successfully instead of being refused with ValidationException.
AWS: Smithy opensearch#DescribeInstanceTypeLimits is GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType} with required httpLabel members InstanceType (OpenSearchPartitionInstanceType enum) and EngineVersion (VersionString, @length min 14 max 18, pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$); its modeled errors are BaseException 400, InternalException 500, InvalidTypeException 409, LimitExceededException 409, ResourceNotFoundException 409 and ValidationException 400. A value such as 'OpenSearch_99.99' SATISFIES the modeled pattern and length, so the pattern alone does not refuse it: the engine version must additionally be resolved against the versions the service actually offers (the same set ListVersions returns), and when it resolves to nothing the call is refused with HTTP 400, Error.Code 'ValidationException'. Per the recorded behavior the message is 'Unsupported Version: <EngineVersion>, allowed Versions are: [<comma-separated engine versions the service offers>]' — fixed text 'Unsupported Version: ' and ', allowed Versions are: ['/']'; the first variable part is the EngineVersion taken verbatim from the request URI, the second is the service's current engine-version catalog (state-derived, grows with each release, and must include the versions the emulator's own ListVersions advertises, e.g. OpenSearch_2.11 when that version is offered). The check's positive legs (ListVersions returning OpenSearch_* versions, ListInstanceTypeDetails(EngineVersion='OpenSearch_2.11') listing t3.small.search, and DescribeInstanceTypeLimits(InstanceType='t3.small.search', EngineVersion='OpenSearch_2.11') returning LimitsByRole) all PASSED, so only the unknown-engine refusal diverges.
Emulator: the call with a nonexistent engine version raises no error at all — the replay records 'bogus EngineVersion should be rejected, got None' (the harness sets code=None only when the SDK call returns successfully), i.e. a 2xx with LimitsByRole where AWS answers 400 ValidationException.
Why: a client cannot tell a supported engine version from an unsupported one. Capacity planners, engine-upgrade tooling and anything that probes the catalog by asking for limits read fabricated limits for an engine AWS does not have, and a negative test for a bad/typo'd engine version passes against the emulator and fails against AWS. It also makes the catalog APIs mutually inconsistent: ListVersions advertises one set of engines while DescribeInstanceTypeLimits answers for any syntactically valid string.
Fix: EngineVersion resolution/validation in the OpenSearch service crate's DescribeInstanceTypeLimits handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_instance_type_limits.rs) — resolve the label against the engine-version catalog that backs ListVersions and raise ValidationException (400) with the 'Unsupported Version: ...' text when it is unknown. This is distinct from the open issue 20261025T185727Z-cloudformation-opensearch-sns-opensearch-describeinstancetypelimits-the-invalidtypeexception-message-for-an-instance.md, which is about the InvalidTypeException (409) MESSAGE for an instance type that exists but is unsupported by a REAL engine version; that cause does not reach the unknown-engine path at all.
Traces: opensearch.instance-catalog-enum-semantics.

Cause 2 of 2: StartServiceSoftwareUpdate on an EXISTING domain that has no service-software update available answers NotImplemented instead of its modeled ValidationException.
AWS: Smithy opensearch#StartServiceSoftwareUpdate is POST /2021-01-01/opensearch/serviceSoftwareUpdate/start with required DomainName, and models exactly BaseException 400, InternalException 500, ResourceNotFoundException 409 and ValidationException 400 — 'NotImplemented' is not in its error set and no 501 is part of its contract. When the domain exists but its DomainStatus.ServiceSoftwareOptions.UpdateAvailable is false there is nothing to deploy, so the request is refused with HTTP 400 and Error.Code 'ValidationException' (that classification is what the check grades). Hypothesis (neither the model nor this recording states the wording): AWS words it as a 'no software update available for this domain' sentence whose only variable part is the domain name from the request; the emulator's ServiceSoftwareOptions.Description already carries the equivalent 'There is no software update available for this domain.' text.
Emulator: the call returns Error.Code 'NotImplemented' — replay output 'AssertionError: expected ValidationException, got NotImplemented' — on a domain the emulator itself created and reported ACTIVE with ServiceSoftwareOptions={'CurrentVersion':'R20250101','UpdateAvailable':False,'UpdateStatus':'COMPLETED', ...}. Because the emulator always reports UpdateAvailable=false, this is the ONLY branch of the operation a client can reach, so StartServiceSoftwareUpdate is effectively unusable: neither the update path nor its refusal exists.
Why: a client cannot distinguish 'this domain has no update to apply' (a 400 it should accept and stop on) from 'the emulator has no handler' (a 501 that is not in the contract at all). Error-code branching, retry/backoff policy and negative tests around service-software maintenance all take the wrong path, and botocore cannot map the unmodeled code to a modeled exception class, so clients that catch ValidationException do not catch it. The downstream UpgradeDomain/RollbackServiceSoftwareUpdate legs of the same workflow are never reached.
Fix: the StartServiceSoftwareUpdate handler in the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/start_service_software_update.rs) — resolve the domain, and when ServiceSoftwareOptions.UpdateAvailable is false return ValidationException (HTTP 400) with the no-update-available message naming the requested domain, instead of NotImplemented. Related but NOT the same as Cause 6 of the open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md, which covers StartServiceSoftwareUpdate on a domain that does NOT exist (must be ResourceNotFoundException); this cause is the existing-domain/no-update-available branch of the same handler and needs its own refusal.
Traces: opensearch.service-software-rollback-and-domain-upgrade.

## Evidence
Cause 1 — opensearch.instance-catalog-enum-semantics (mismatch):
  AssertionError: bogus EngineVersion should be rejected, got None
  (check_opensearch_instance_catalog_enum_semantics.py line 32: assert code == "ValidationException")
  Call: opensearch.describe_instance_type_limits(InstanceType='t3.small.search', EngineVersion='OpenSearch_99.99')
  Expected (AWS): HTTP 400, Error.Code 'ValidationException', Error.Message starting with 'Unsupported Version: OpenSearch_99.99, allowed Versions are: [' and ending with ']', with the bracketed list containing the engine versions the service offers (the check asserts OpenSearch_2.11 is among them because the same emulator advertises it).
  Actual (emulator): the call SUCCEEDS — no ClientError is raised (code None), so a 2xx response with LimitsByRole is returned for an engine version that does not exist.
  Positive controls in the same check passed: list_versions() returned OpenSearch_* versions; list_instance_type_details(EngineVersion='OpenSearch_2.11') listed t3.small.search; describe_instance_type_limits('t3.small.search','OpenSearch_2.11') returned LimitsByRole.

Cause 2 — opensearch.service-software-rollback-and-domain-upgrade (mismatch):
  AssertionError: expected ValidationException, got NotImplemented
  (check_opensearch_service_software_rollback_and_domain_upgrade.py line 85: assert code == "ValidationException", reached via the `else` branch taken because describe_domain reported ServiceSoftwareOptions.UpdateAvailable == False)
  Call: opensearch.start_service_software_update(DomainName=<domain the check created and waited ACTIVE>)
  Expected (AWS): HTTP 400 with Error.Code 'ValidationException' (StartServiceSoftwareUpdate models ValidationException 400 / ResourceNotFoundException 409 only).
  Actual (emulator): Error.Code 'NotImplemented' (501), a code outside the operation's modeled error set.
  Corroborating state from the same emulator (DescribeDomain DomainStatus in this suite): 'ServiceSoftwareOptions': {'CurrentVersion': 'R20250101', 'UpdateAvailable': False, 'Cancellable': False, 'UpdateStatus': 'COMPLETED', 'Description': 'There is no software update available for this domain.', 'OptionalDeployment': True} — so the no-update-available branch is the only reachable one.

Smithy grounding: opensearch#DescribeInstanceTypeLimits errors = BaseException 400, InternalException 500, InvalidTypeException 409, LimitExceededException 409, ResourceNotFoundException 409, ValidationException 400; EngineVersion is a required httpLabel of VersionString (length 14..18, pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$) which 'OpenSearch_99.99' matches. opensearch#StartServiceSoftwareUpdate errors = BaseException 400, InternalException 500, ResourceNotFoundException 409, ValidationException 400 (no NotImplemented, no 501).

## Steps to Reproduce
Cause 1:
1. os = boto3.client('opensearch', endpoint_url=<emulator>, region_name='us-west-2')
2. os.list_versions() -> note the OpenSearch_* versions advertised (includes OpenSearch_2.11).
3. os.describe_instance_type_limits(InstanceType='t3.small.search', EngineVersion='OpenSearch_2.11') -> 200 with LimitsByRole (positive control, passes today).
4. try: os.describe_instance_type_limits(InstanceType='t3.small.search', EngineVersion='OpenSearch_99.99')
   except ClientError as e: print(e.response['ResponseMetadata']['HTTPStatusCode'], e.response['Error']['Code'], repr(e.response['Error']['Message']))
   Observed: no exception — the call returns 200.
   Expected: 400 ValidationException with Message 'Unsupported Version: OpenSearch_99.99, allowed Versions are: [<engine versions offered, including OpenSearch_2.11>]'.

Cause 2:
1. name = 'sim2real-osb2-<suffix>'; os.create_domain(DomainName=name, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, AccessPolicies=<account-root es:* policy>)
2. Poll os.describe_domain(DomainName=name)['DomainStatus'] until Created is true and Processing is false; observe ServiceSoftwareOptions['UpdateAvailable'] is False.
3. try: os.start_service_software_update(DomainName=name)
   except ClientError as e: print(e.response['ResponseMetadata']['HTTPStatusCode'], e.response['Error']['Code'], repr(e.response['Error']['Message']))
   Observed: Error.Code 'NotImplemented' (501).
   Expected: 400 ValidationException reporting that no software update is available for domain <name>.

Replay: opensearch.instance-catalog-enum-semantics, opensearch.service-software-rollback-and-domain-upgrade
