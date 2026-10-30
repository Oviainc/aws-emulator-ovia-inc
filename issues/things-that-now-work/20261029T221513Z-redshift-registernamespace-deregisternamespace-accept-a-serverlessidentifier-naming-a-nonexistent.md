Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.namespace-error-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.namespace-error-semantics|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# RegisterNamespace/DeregisterNamespace accept a ServerlessIdentifier naming a nonexistent serverless namespace/workgroup (HTTP 200 'Registering') instead of ClusterNotFound

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterNamespace

## Description
One net-new root cause in the redshift service: the serverless arm of the NamespaceIdentifier union is never resolved, so a registration is accepted for a serverless namespace/workgroup pair that does not exist.

Cause 1 of 1: the ServerlessIdentifier arm of RegisterNamespace/DeregisterNamespace resolves nothing and always succeeds. Failing check id: redshift.namespace-error-semantics.

AWS: RegisterNamespace and DeregisterNamespace resolve the NamespaceIdentifier union through one shared target-resolution path, for BOTH arms. For a ProvisionedIdentifier whose ClusterIdentifier does not exist, and equally for a ServerlessIdentifier whose NamespaceIdentifier/WorkgroupIdentifier pair does not exist, the call is refused with the cluster-not-found fault: Smithy (com.amazonaws.redshift#RegisterNamespace / #DeregisterNamespace) models ClusterNotFoundFault with errorKind client, httpError 404 and awsQueryError code `ClusterNotFound`, httpResponseCode 404; the input member NamespaceIdentifier targets NamespaceIdentifierUnion and ConsumerIdentifiers is a required list @length 1..1. The model does not spell out the serverless message, so its wording is recorded behavior only: the fixed text is `Could not find serverless workgroup for serverless workgroup owner "` + ... + `" with serverless workgroup name "` + ... + `"`, where the owner is the caller's own account id (from the credentials/the account that owns the request) and the workgroup name is the WorkgroupIdentifier taken verbatim from the request — neither is a constant and neither may be hard-coded from any single run. (The provisioned arm's message is the separate fixed text "Cluster " + the requested ClusterIdentifier + " can't be found.") InvalidNamespaceFault is NOT the fault for this case: it belongs to AssociateDataShareConsumer's own namespace-reference validation, a different code path.

Emulator: the provisioned arm now refuses correctly, but the serverless arm is not resolved at all. RegisterNamespace with NamespaceIdentifier={'ServerlessIdentifier': {'NamespaceIdentifier': <nonexistent>, 'WorkgroupIdentifier': <nonexistent>}} returns HTTP 200 with {'Status': 'Registering'} and pushes a StoredNamespaceRegistration for that pair. The handler comment states the behavior outright: "the serverless arm of its key is any identifier a caller sends" (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/register_namespace.rs:205-211), with the only bound being an emulator-invented InvalidParameterValue quota on the number of stored registrations. DeregisterNamespace shares the same resolution helper (store.registration_target) and therefore the same defect; the check never reaches its DeregisterNamespace serverless assertion because the RegisterNamespace one fails first.

Why: an over-permissive accept. A client wiring a Glue Data Catalog registration against a serverless namespace that does not exist (typo'd namespace or workgroup name, wrong region, workgroup deleted) is told the registration is under way, so its error handling for ClusterNotFound/404 is dead code and every downstream read of that registration describes a namespace that has no backing resource. Code that distinguishes "target missing" from "target busy" cannot be exercised against the emulator at all.

Fix: the shared namespace-registration target resolution used by the RegisterNamespace and DeregisterNamespace handlers (store.registration_target in the redshift service state, reached from src/ops/register_namespace.rs and src/ops/deregister_namespace.rs) must resolve the ServerlessIdentifier arm against redshift-serverless namespace/workgroup state and return the ClusterNotFound refusal (404, awsQueryError code ClusterNotFound) with the serverless wording above when the pair is absent, instead of admitting any identifier a caller sends.

Note: open issue 20261029T200555Z-redshift-namespace-reserved-node-idc-application-hsm-refusals.md lists this check id, but its replay key for it is the PROVISIONED arm's message assertion (register_cluster_message == cluster_missing), which no longer fails; it carries no key for this unresolved-serverless-target accept, so this cause is not covered by it.

Traces: redshift.namespace-error-semantics.

## Evidence
redshift.namespace-error-semantics, check_redshift_namespace_error_semantics.py line 71 -> _error() line 124:

AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Status': 'Registering', 'ResponseMetadata': {'RequestId': ..., 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}, 'RetryAttempts': 0}}

The failing call is RegisterNamespace(NamespaceIdentifier={'ServerlessIdentifier': {'NamespaceIdentifier': <minted nonexistent name>, 'WorkgroupIdentifier': <same nonexistent name>}}, ConsumerIdentifiers=[<caller account id>]).

Expected: ClientError, HTTP 404, Error.Code 'ClusterNotFound', Error.Message with fixed text 'Could not find serverless workgroup for serverless workgroup owner "<owner account id>" with serverless workgroup name "<WorkgroupIdentifier from the request>"' (the two quoted values come from the caller's account and the request; the rest is fixed text).
Actual: HTTP 200, {'Status': 'Registering'}.

The preceding assertion in the same check — the ProvisionedIdentifier arm with a nonexistent ClusterIdentifier, expecting ClusterNotFound + 'Cluster <id> can\'t be found.' — passed, which localises the defect to the serverless arm of the shared resolution path. The same check's later DeregisterNamespace serverless assertion is unreached.

Corroborating source: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/register_namespace.rs:190-218 resolves the target via store.registration_target and then, for a target it did not find, pushes a registration, bounded only by an emulator-invented NAMESPACE_REGISTRATION_QUOTA InvalidParameterValue; the comment at lines 205-207 states the serverless key is "any identifier a caller sends".

## Steps to Reproduce
1. Boot the emulator and point a botocore redshift client at it.
2. Get the caller account id: sts.get_caller_identity()['Account'].
3. Negative, serverless arm — RegisterNamespace(NamespaceIdentifier={'ServerlessIdentifier': {'NamespaceIdentifier': 'sim2real-nsns-badns-<random>', 'WorkgroupIdentifier': 'sim2real-nsns-badns-<random>'}}, ConsumerIdentifiers=[<account id>]).
   Expect: ClientError, HTTP 404, Error.Code 'ClusterNotFound', Error.Message 'Could not find serverless workgroup for serverless workgroup owner "<account id>" with serverless workgroup name "sim2real-nsns-badns-<random>"'.
   Observed: HTTP 200 with {'Status': 'Registering'}.
4. Repeat step 3 against DeregisterNamespace with the same ServerlessIdentifier — same expectation, same shared resolution path.
5. Control (must keep working): the ProvisionedIdentifier arm with a nonexistent ClusterIdentifier must still fail ClusterNotFound/404 with 'Cluster <id> can\'t be found.', and RegisterNamespace/DeregisterNamespace against a real available cluster must still return Status Registering/Deregistering.
6. Replay redshift.namespace-error-semantics to confirm.
