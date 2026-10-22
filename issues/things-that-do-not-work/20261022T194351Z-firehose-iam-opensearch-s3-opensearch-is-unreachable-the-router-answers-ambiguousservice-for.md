Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3 firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3
Issue-Key: firehose__iam__opensearch__s3|CreateDomain|AmbiguousService
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# OpenSearch is unreachable: the router answers AmbiguousService for every request signed for the 'es' endpoint prefix

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: two emulated services both claim the 'es' endpoint prefix, so the router refuses every OpenSearch/Elasticsearch request instead of serving it.
AWS: opensearch.CreateDomain is an ordinary control-plane call signed with the sigv4 service name 'es' against the es.<region>.amazonaws.com endpoint; it returns 200 with a DomainStatus (and modeled client errors such as ValidationException/ResourceAlreadyExistsException for bad input). There is exactly one service behind that prefix from a client's point of view.
Emulator: the request is rejected before any handler runs — 'An error occurred (AmbiguousService) when calling the CreateDomain operation (reached max retries: 3): 2 emulated services all claim requests for "es": es, es; the request is rejected rather than served by an arbitrary one'. Because botocore retries the error, every call also burns the retry budget. The AmbiguousService code is not an AWS error code at all.
Why: the entire OpenSearch surface is unreachable for any client: a domain cannot be created, so nothing downstream (Firehose delivery to an OpenSearch destination, FGAC role mapping, the routing of rejected documents to the S3 error prefix) can be exercised or used. The emulator announces an endpoint it then refuses to serve.
Fix: endpoint/service routing — the two registrations claiming 'es' (the OpenSearch service and the legacy Elasticsearch Service crate) must be disambiguated so one owns the prefix and the request is dispatched to it, in the request-router/service-registry layer rather than in either operation handler.
Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-routes-conflicts-to-s3, firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-unmapped-role-denied-all-records-to-s3.

## Evidence
Expected: 200 with a DomainStatus from opensearch.create_domain. Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateDomain operation (reached max retries: 3): 2 emulated services all claim requests for 'es': es, es; the request is rejected rather than served by an arbitrary one. Both checks in this group fail identically on their first OpenSearch call.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. os = client('opensearch'); os.create_domain(DomainName='sim2real-dom', ClusterConfig={...}, EBSOptions={...}) -> expect 200 with DomainStatus.
3. Observe the request is refused with AmbiguousService naming two services claiming 'es', after botocore exhausts its retries.
4. Re-replay the check ids in trace_ids.
