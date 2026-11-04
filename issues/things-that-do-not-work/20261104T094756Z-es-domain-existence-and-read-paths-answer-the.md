Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-create-domain-gp3-server-defaults
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-create-domain-gp3-server-defaults opensearch-domain-lifecycle-and-config-inspection esval-ebs-gp3-iops-below-floor esval-ebs-gp3-throughput-below-floor esval-range-snapshot-start-hour-25 esval-topology-standby-single-data-node esval-topology-three-az-two-data-nodes opensearch-domain-insights-operations
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->4xx|$.DomainStatus
Issue-Key: es|CreateDomain|2xx->4xx|status
Issue-Key: es|DeleteDomain|4xx->2xx|status
Issue-Key: es|DescribeDomains|2xx->2xx|$.DomainStatusList[*]
Issue-Key: es|DescribeInsightDetails|2xx->4xx|$.Fields
Issue-Key: es|DescribeInsightDetails|2xx->4xx|status
Issue-Commit: 8f194cc1fc794f910c53fc884e414c1833919c6c

# es: domain existence and read paths answer the wrong status (4 causes)

> _Note: this issue was identified against the code base at commit `8f194cc1fc794f910c53fc884e414c1833919c6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomain

## Description
Four independent defects on the emulator's OpenSearch/Elasticsearch (`es` endpoint prefix) control plane make a domain's existence answer the wrong way: a create that AWS answers 200 is refused 409, a delete of a domain that does not exist succeeds 200 instead of being refused, a read of an existing domain comes back with an empty list, and an insight read is refused 404 instead of answering its informational payload. This bundle carries FOUR causes and all four must be closed before it is submitted.

Cause 1 of 4: CreateDomain answers 409 where the recording shows 200 with the domain's DomainStatus.
AWS: recorded behavior only — at this step the recording shows CreateDomain (POST /2021-01-01/opensearch/domain) answering HTTP 200 with a complete DomainStatus for the domain (Created=true, Deleted=false, ChangeProgressDetails.ConfigChangeStatus 'Completed'). Smithy models CreateDomain's 409 classes as DisabledOperationException, InvalidTypeException, LimitExceededException and ResourceAlreadyExistsException, so a 409 here asserts one of those conditions.
Emulator: answers HTTP 409 with no DomainStatus at all, so the whole response body is missing. Hypothesis: the emulator refuses the create because it already holds a domain record for that name (ResourceAlreadyExists-style refusal), where the recording shows the service answering the step with 200 and the domain's status.
Why: a client's create step fails outright — every later step that depends on the domain (describe, tag, update, delete) is unreachable, which is why this is the most expensive of the four.
Fix: the CreateDomain handler's pre-flight existence/limit check and the 409 it raises there.
Traces: esdef-create-domain-gp3-server-defaults#task#3 (and #task#4), opensearch-domain-lifecycle-and-config-inspection#task#14.

Cause 2 of 4: DeleteDomain on a domain that does not exist answers 200 instead of refusing it.
AWS: DeleteDomain (DELETE /2021-01-01/opensearch/domain/{DomainName}) raises ResourceNotFoundException for a name the account does not hold; the recordings for these steps show HTTP 404 with that error class (Smithy lists ResourceNotFoundException among DeleteDomain's errors — note this corpus records the service answering 404 for it, which is the status asserted here; the error message is not asserted by this step, so the requirement here is the status and the ResourceNotFoundException class).
Emulator: answers HTTP 200 and reports a successful delete for a domain it never created.
Why: a client cannot distinguish "deleted" from "never existed", so a teardown reports success for state it did not own and an idempotency check built on the not-found refusal silently passes.
Fix: the existence check at the top of the DeleteDomain handler (refuse with ResourceNotFoundException before mutating state). Note these steps are teardowns of validation fixtures whose CreateDomain was wrongly ACCEPTED by the already-filed over-accepting-create issue 20261029T190009Z-es-ten-requests-real-aws-refuses-are-accepted.md; the delete path must refuse an absent domain on its own, independent of that fix.
Traces: esval-ebs-gp3-iops-below-floor#teardown#2, esval-ebs-gp3-throughput-below-floor#teardown#2, esval-range-snapshot-start-hour-25#teardown#2, esval-topology-standby-single-data-node#teardown#2, esval-topology-three-az-two-data-nodes#teardown#2.

Cause 3 of 4: DescribeDomains answers 200 with a DomainStatusList that does not contain the requested, existing domain.
AWS: DescribeDomains returns one DomainStatus element per requested DomainNames entry that exists; the recording shows the list carrying the domain created earlier in the same fixture, with its full status.
Emulator: answers 200 with no element at index 0 — the requested domain is absent from DomainStatusList even though DescribeDomain for the same name in the same fixture answers with it.
Why: a client that enumerates domains in bulk (console-style listing, inventory, drift detection) concludes the domain does not exist and skips or recreates it, even though the single-domain read sees it.
Fix: the DomainNames lookup/filter in the DescribeDomains handler (it must resolve names against the same stored domain records DescribeDomain uses).
Traces: opensearch-domain-lifecycle-and-config-inspection#task#3.

Cause 4 of 4: DescribeInsightDetails refuses every insight id instead of answering its informational Fields list.
AWS: DescribeInsightDetails (POST /2021-01-01/opensearch/insight-details) answers HTTP 200 with the @required Fields list; the recording shows a single informational field {Name: 'Message', Type: 'text', Value: 'Please use the Amazon OpenSearch Console to view the Insight Details for a better user experience.'} — fixed text, with no value interpolated from the request.
Emulator: answers HTTP 404 (not-found refusal) and returns no Fields member at all.
Why: Fields is @required in the response shape, so a generated client gets a refusal where the service always answers successfully; any insight-detail read fails. An open issue describes the same refusal (20261104T090541Z-opensearch-describeinsightdetails-refuses-every-insight-id-with-resourcenotfoundexception.md) but does not carry this fixture step's replay key, so this cause is filed here with its key.
Fix: the DescribeInsightDetails handler — return 200 with the informational Fields list rather than a resource-not-found refusal.
Traces: opensearch-domain-insights-operations#task#3.

## Evidence
Replayed against the emulator booted from this checkout; service `es`.

Cause 1 — esdef-create-domain-gp3-server-defaults#task#3 (CreateDomain): status expected 200, got 409; $.DomainStatus missing (expected the full DomainStatus object with ARN 'arn:aws:es:<region>:<account>:domain/<domain-name>', Created=true, Deleted=false, ChangeProgressDetails.ConfigChangeStatus 'Completed', ClusterConfig InstanceCount 1 / InstanceType 't3.small.search', got None). Cluster also covers esdef-create-domain-gp3-server-defaults#task#4 and opensearch-domain-lifecycle-and-config-inspection#task#14.

Cause 2 — esval-ebs-gp3-iops-below-floor#teardown#2 (DeleteDomain): status expected 404, got 200. Cluster also covers esval-ebs-gp3-throughput-below-floor#teardown#2, esval-range-snapshot-start-hour-25#teardown#2, esval-topology-standby-single-data-node#teardown#2, esval-topology-three-az-two-data-nodes#teardown#2 (5 steps).

Cause 3 — opensearch-domain-lifecycle-and-config-inspection#task#3 (DescribeDomains): status 200 as expected, but $.DomainStatusList[0] missing (expected the DomainStatus for the fixture's domain, got None).

Cause 4 — opensearch-domain-insights-operations#task#3 (DescribeInsightDetails): status expected 200, got 404; $.Fields missing (expected [{'Name': 'Message', 'Type': 'text', 'Value': 'Please use the Amazon OpenSearch Console to view the Insight Details for a better user experience.'}], got None).

Model references: services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json — CreateDomain errors {DisabledOperationException, InvalidTypeException, LimitExceededException, ResourceAlreadyExistsException, ValidationException, BaseException, InternalException}; DeleteDomain errors {ResourceNotFoundException, ValidationException, BaseException, InternalException}; DescribeInsightDetails output Fields is @required (target InsightFieldList).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay esdef-create-domain-gp3-server-defaults through task#3 — the CreateDomain step answers 409 with no body where the recording answers 200 with DomainStatus.
3. Cause 2: replay esval-ebs-gp3-iops-below-floor#teardown#2 — DELETE /2021-01-01/opensearch/domain/<name> for a domain the account does not hold answers 200 instead of 404 ResourceNotFoundException.
4. Cause 3: replay opensearch-domain-lifecycle-and-config-inspection#task#3 — DescribeDomains naming the domain created earlier in the fixture answers 200 with an empty DomainStatusList, while DescribeDomain for the same name answers with it.
5. Cause 4: replay opensearch-domain-insights-operations#task#3 — POST /2021-01-01/opensearch/insight-details answers 404 instead of 200 with the informational Fields list.
