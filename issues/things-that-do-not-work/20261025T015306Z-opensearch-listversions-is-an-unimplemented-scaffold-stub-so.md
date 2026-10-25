Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back
Issue-Key: cloudformation__opensearch__sns|ListVersions|NotImplemented
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# OpenSearch ListVersions is an unimplemented scaffold stub, so engine-version validation of a domain cannot be exercised (1 cause)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVersions

## Description
Cause 1 of 1: opensearch ListVersions answers NotImplemented for every request.

AWS: ListVersions returns the list of OpenSearch/Elasticsearch engine versions the service supports (paginated via NextToken/MaxResults). It is the catalog a caller consults to pick a valid EngineVersion and to decide whether an instance type is newer than the engine it is being paired with.
Emulator: every call is refused with 'NotImplemented' — 'operation ListVersions is scaffolded but not implemented in this emulator' — so the check aborts before it can assert that an instance type newer than the engine is refused and the sibling SNS topic rolled back.
Why: the engine-version catalog is empty from a client's point of view, so version-aware provisioning logic cannot run and the refusal is not one of the operation's modeled errors.
Fix: implement the ListVersions handler in the opensearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/list_versions.rs) over a static supported-version catalog, with NextToken paging.
Traces: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back. (The sibling check in that group is blocked by the already-filed DescribeInstanceTypeLimits stub; this one is a different operation.)

## Evidence
Replayed suite, check cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back:
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListVersions operation: operation ListVersions is scaffolded but not implemented in this emulator"

## Steps to Reproduce
1. Boot the emulator and replay the suite; re-run cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back.
2. Manual repro: call opensearch ListVersions() — AWS returns a Versions list, the emulator answers NotImplemented.
