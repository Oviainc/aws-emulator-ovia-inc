Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.endpoint-access-ops-not-found-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.endpoint-access-ops-not-found-semantics|run#11|AssertionError|assert observed == expected or observed == {
Issue-Commit: 11e424e87637980d781b5ebada45988cd752adfb

# Redshift-managed VPC endpoint (endpoint-access) operations all answer NotImplemented instead of their modeled not-found errors (1 cause)

> _Note: this issue was identified against the code base at commit `11e424e87637980d781b5ebada45988cd752adfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpointAccess

## Description
Cause 1 of 1: the whole Redshift-managed VPC endpoint (endpoint-access) operation family is a scaffold that answers the generic `NotImplemented` error, so none of the operations resolve an endpoint, a subnet group or a cluster and none of them raise the error AWS models for an unresolvable name.

Related open issue: 20261024T024248Z-redshift-cluster-scoped-credential-and-notification-operations-answer.md describes the same family as its Cause 9, but it does not carry this check's current replay key (its Issue-Key for this check names an older assertion form, `run#11`, while the check now fails at `run` line 94 with `assert observed == expected or observed == {...}`), so the suite does not report this failure as owned and it is filed here as its own work item. A fixer closing either must make the family answer modeled errors.

AWS: each operation in the family distinguishes an unknown ENDPOINT from an unknown CLUSTER or SUBNET GROUP, and refuses with a modeled fault.
Smithy (com-amazonaws-redshift-redshiftserviceversion20121201):
  * DescribeEndpointAccess / DeleteEndpointAccess / ModifyEndpointAccess model `EndpointNotFoundFault` (errorKind client, httpError 404, awsQueryError code `EndpointNotFound`, 404) alongside `ClusterNotFoundFault` (`ClusterNotFound`, 404) and `InvalidClusterStateFault` (`InvalidClusterState`, 400). For an EndpointName that does not exist the recorded AWS response is error code `EndpointNotFound`.
  * CreateEndpointAccess models `ClusterSubnetGroupNotFoundFault` (awsQueryError code `ClusterSubnetGroupNotFoundFault`, 400) and `ClusterNotFoundFault` (`ClusterNotFound`, 404). The recording shows AWS resolving the subnet group first, so a request naming both an unknown subnet group and an unknown cluster is refused `ClusterSubnetGroupNotFoundFault`; no precedence is documented, so `ClusterNotFound` is the only other acceptable refusal (recorded behavior only).
  * AuthorizeEndpointAccess / RevokeEndpointAccess / DescribeEndpointAuthorization model `ClusterNotFoundFault` (`ClusterNotFound`, 404); against a ClusterIdentifier that does not exist AWS refuses with `ClusterNotFound`.

Emulator: all seven operations answer error code `NotImplemented` (the scaffold's "operation <X> is scaffolded but not implemented in this emulator" channel) regardless of which name in the request is unresolvable, so the endpoint/cluster/subnet-group distinction does not exist and there is no success path either.

Why: a client cannot tell "no such endpoint" from "no such cluster" from "the call is broken". `NotImplemented` is not in any of these operations' modeled error sets, so SDK error matching (catch EndpointNotFoundFault / ClusterNotFoundFault), SDK retry classifiers and waiters all treat it as an unknown non-retryable failure; idempotent private-connectivity setup ("create the endpoint unless it already exists", "delete it if present") has neither a refusal to branch on nor a working create, so no Redshift-managed VPC endpoint flow can be exercised at all.

Fix: the endpoint-access operation handlers in the redshift service's ops directory (describe_endpoint_access, delete_endpoint_access, modify_endpoint_access, create_endpoint_access, authorize_endpoint_access, revoke_endpoint_access, describe_endpoint_authorization) plus an endpoint / endpoint-authorization registry in the service state they look names up in: resolve the endpoint, then the subnet group, then the cluster, and raise the corresponding modeled fault with its awsQuery code and HTTP status before any other work.

Traces: redshift.endpoint-access-ops-not-found-semantics.

## Evidence
Failing check redshift.endpoint-access-ops-not-found-semantics (verdict: mismatch), check_redshift_endpoint_access_ops_not_found_semantics.py line 94:

AssertionError: endpoint-access contract drift:
 expected={'DescribeEndpointAccess': 'EndpointNotFound', 'DeleteEndpointAccess': 'EndpointNotFound', 'ModifyEndpointAccess': 'EndpointNotFound', 'CreateEndpointAccess': 'ClusterSubnetGroupNotFoundFault', 'AuthorizeEndpointAccess': 'ClusterNotFound', 'RevokeEndpointAccess': 'ClusterNotFound', 'DescribeEndpointAuthorization': 'ClusterNotFound'}
 observed={'DescribeEndpointAccess': 'NotImplemented', 'DeleteEndpointAccess': 'NotImplemented', 'ModifyEndpointAccess': 'NotImplemented', 'CreateEndpointAccess': 'NotImplemented', 'AuthorizeEndpointAccess': 'NotImplemented', 'RevokeEndpointAccess': 'NotImplemented', 'DescribeEndpointAuthorization': 'NotImplemented'}

Expected per the Smithy model: EndpointNotFoundFault -> awsQueryError code `EndpointNotFound`, HTTP 404; ClusterSubnetGroupNotFoundFault -> code `ClusterSubnetGroupNotFoundFault`, HTTP 400; ClusterNotFoundFault -> code `ClusterNotFound`, HTTP 404. Observed: code `NotImplemented` for every one of the seven calls (the recorded message text is an example of the scaffold channel, not the requirement; the status and error code are).

## Steps to Reproduce
1. Boot the emulator and point a redshift client at it.
2. Call DescribeEndpointAccess, DeleteEndpointAccess and ModifyEndpointAccess with an EndpointName that was never created. Expect error code `EndpointNotFound`, HTTP 404; observe `NotImplemented`.
3. Call CreateEndpointAccess with an unknown EndpointName, an unknown SubnetGroupName and an unknown ClusterIdentifier. Expect `ClusterSubnetGroupNotFoundFault` (HTTP 400), or `ClusterNotFound` (HTTP 404); observe `NotImplemented`.
4. Call AuthorizeEndpointAccess and RevokeEndpointAccess (Account=111122223333) and DescribeEndpointAuthorization with a ClusterIdentifier that does not exist. Expect `ClusterNotFound`, HTTP 404; observe `NotImplemented`.
5. Re-replay behavioral check redshift.endpoint-access-ops-not-found-semantics to confirm the fix.
