Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-dns-namespace-request-validation-errors
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-dns-namespace-request-validation-errors
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.ErrorCode
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.ErrorMessage
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.Status
Issue-Key: servicediscovery|GetNamespace|4xx->2xx|$.__type
Issue-Key: servicediscovery|GetNamespace|4xx->2xx|status
Issue-Related: 20261029T203804Z-servicediscovery-cloud-map-namespace-service-and-operation-responses-omit.md
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map CreatePrivateDnsNamespace operation reports SUCCESS where AWS fails with CANNOT_CREATE_HOSTED_ZONE

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperation

## Description
TWO net-new wire divergences in the servicediscovery (Cloud Map) emulator, both from one request: a CreatePrivateDnsNamespace whose hosted zone cannot be created. Its asynchronous operation lands SUCCESS instead of FAIL, and because of that the namespace is left readable where AWS has none. Both causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Cause 1 of 2: GetOperation reports Status SUCCESS (and no error members, and no OwnerAccount) for a private-DNS-namespace creation that AWS terminates as FAIL with ErrorCode CANNOT_CREATE_HOSTED_ZONE.
AWS: Smithy CreatePrivateDnsNamespace{Name (required), CreatorRequestId, Description, Vpc (required), Tags, Properties} -> OperationId; GetOperation{OperationId (required), OwnerAccount} -> Operation{Id, OwnerAccount: AWSAccountId, Type, Status: OperationStatus (SUBMITTED|PENDING|SUCCESS|FAIL), ErrorMessage, ErrorCode, CreateDate, UpdateDate, Targets}. Operation$ErrorCode documents CANNOT_CREATE_HOSTED_ZONE as one of its values, and Operation$Status documents that FAIL means 'the operation failed - for the failure reason, see ErrorMessage'. Recorded: the synchronous call returns an OperationId, and the operation reaches terminal Status FAIL with ErrorCode 'CANNOT_CREATE_HOSTED_ZONE', a non-empty ErrorMessage, and OwnerAccount set to the owning account - i.e. the hosted-zone creation Cloud Map must perform in Route 53 for a private DNS namespace cannot be completed for this request, and the namespace is therefore never created.
Emulator: GetOperation polls to Status 'SUCCESS' with no ErrorCode, no ErrorMessage and no OwnerAccount member; the replay's poll aborts because SUCCESS is in the step's fail_on set.
Why: the async failure channel of this operation is unobservable - a client that polls GetOperation to learn whether its private namespace really exists is told it succeeded, so it proceeds to use a namespace whose DNS plumbing does not exist, and every test of FAIL/ErrorCode handling passes vacuously. Related but distinct: the open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md describes the same SUCCESS-instead-of-FAIL family in its Cause 4 for a nonexistent Vpc; this wire step reaches it through the hosted-zone creation path with a distinct ErrorCode and is a separate replay key, so closing that issue does not close this.
Fix: CreatePrivateDnsNamespace's async-operation state machine (the handler plus the operation store that advances operations) - the hosted-zone creation precondition must be evaluated when the operation is advanced, and the operation must land FAIL carrying ErrorCode CANNOT_CREATE_HOSTED_ZONE and an ErrorMessage, with OwnerAccount populated.
Traces: servicediscovery-dns-namespace-request-validation-errors#task#5.

Cause 2 of 2: GetNamespace returns the namespace (HTTP 200) for a creation that AWS never completed, instead of the NamespaceNotFound error.
AWS: Smithy GetNamespace{Id (required)} -> Namespace; errors InvalidInput (400), NamespaceNotFound (modelled @httpError 404, but recorded on the wire as HTTP 400 with the code in the awsJson1_1 __type field). Recorded: after the creation operation terminates FAIL, reading the namespace the failed operation targeted returns __type 'NamespaceNotFound' - a failed create leaves NO namespace behind.
Emulator: HTTP 200 with a fully formed Namespace, no __type at all.
Why: a failed asynchronous create leaves a phantom resource in the store, so cleanup/retry logic sees a namespace that AWS says does not exist, names stay occupied, and a subsequent create can be refused as already-existing.
Fix: the operation-completion path in the state store - a FAIL terminal state must roll back (or never apply) the namespace record the operation targeted; the same commit-on-success rule applies to the other async create handlers.
Traces: servicediscovery-dns-namespace-request-validation-errors#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. Both clusters come from the servicediscovery-dns-namespace-request-validation-errors fixture (expected = recorded AWS response):

Cause 1 - servicediscovery-dns-namespace-request-validation-errors#task#5, GetOperation (HTTP 200 in both): '$.Operation.Status: value_mismatch (expected \'FAIL\', got \'SUCCESS\')'; '$.Operation.Status: poll: \'SUCCESS\' is in fail_on after 2 attempt(s) (expected \'FAIL\', got \'SUCCESS\')'; '$.Operation.ErrorCode: missing (expected \'CANNOT_CREATE_HOSTED_ZONE\', got None)'; '$.Operation.ErrorMessage: missing (expected a non-empty failure reason, got None)'; '$.Operation.OwnerAccount: missing (expected the owning account id, got None)'.
Cause 2 - servicediscovery-dns-namespace-request-validation-errors#task#6, GetNamespace: 'status: expected 400, got 200'; '$.__type: missing (expected \'NamespaceNotFound\', got None)' - the emulator served the namespace the failed create should never have left behind.

## Steps to Reproduce
Boot the emulator from this checkout and replay servicediscovery-dns-namespace-request-validation-errors steps task#4, task#5 and task#6 in order (task#4 issues the create), or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_private_dns_namespace(Name='<unique>', Vpc='<vpc id>')['OperationId'] using the same request the fixture records (a private DNS namespace whose Route 53 hosted zone cannot be created).
2. Cause 1: poll sd.get_operation(OperationId=op)['Operation'] to a terminal state -> emulator reports Status 'SUCCESS' with no ErrorCode/ErrorMessage/OwnerAccount; AWS reports Status 'FAIL', ErrorCode 'CANNOT_CREATE_HOSTED_ZONE', a non-empty ErrorMessage and OwnerAccount.
3. Cause 2: take the namespace id the operation targeted (Operation['Targets']['NAMESPACE'] or the name you created) and call sd.get_namespace(Id=<that id>) -> emulator 200 with a full Namespace; AWS answers NamespaceNotFound (status 400 on the wire, __type NamespaceNotFound).
