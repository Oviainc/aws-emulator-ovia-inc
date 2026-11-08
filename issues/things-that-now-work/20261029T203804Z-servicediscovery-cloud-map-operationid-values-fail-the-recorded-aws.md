Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-dns-namespace-request-validation-errors
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-dns-namespace-request-validation-errors servicediscovery-http-namespace-service-instance-discovery-lifecycle servicediscovery-instance-registration-error-recovery servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|CreatePrivateDnsNamespace|2xx->2xx|$.OperationId
Issue-Key: servicediscovery|UpdateHttpNamespace|2xx->2xx|$.OperationId
Issue-Key: servicediscovery|RegisterInstance|2xx->2xx|$.OperationId
Issue-Key: servicediscovery|DeregisterInstance|2xx->2xx|$.OperationId
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map OperationId values fail the recorded AWS operation-id shape

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterInstance

## Description
FOUR independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator: the OperationId returned by the asynchronous operations does not satisfy the shape the corpus recorded for AWS's operation ids, so the replay's shape/correlation assertion on that member fails on four different operations. Each of the four causes below is a separate replay key over a different handler and must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact: OperationId is modelled only as a string of length 0..255 (com.amazonaws.servicediscovery#OperationId), so the model states no format - this is recorded behavior only. In the recordings, the OperationId AWS returns is a token the fixture carries forward and re-asserts, and the emulator's value does not match the recorded token shape (the corpus reports the expectation as a shape assertion, '<shaped-not-asserted>', or as a captured placeholder that the emulator's value failed to satisfy). The emulator's value is a bare 32-character lowercase alphanumeric string. Hypothesis: AWS Cloud Map operation ids carry a structured suffix (a hyphen-separated trailing segment after the 32-character body), which the emulator's id generator does not produce; the requirement is the recorded FORM, never one run's literal id.

Cause 1 of 4: CreatePrivateDnsNamespace's OperationId fails the recorded shape.
AWS: Smithy CreatePrivateDnsNamespace{Name (required), CreatorRequestId, Description, Vpc (required), Tags, Properties} -> {OperationId: OperationId}. Recorded: an operation id in AWS's form, which the fixture asserts by shape rather than by literal value.
Emulator: a 32-character lowercase alphanumeric token with no structured suffix, which fails that assertion.
Why: clients that validate or parse the operation id (or tests pinned to its form) diverge; the value is also what every later GetOperation poll is keyed on, so an off-form id is visible on every async flow.
Fix: the operation-id generator in the service state (state.next_id-style allocator) used by the async handlers.
Traces: servicediscovery-dns-namespace-request-validation-errors#task#4.

Cause 2 of 4: UpdateHttpNamespace's OperationId fails the recorded shape.
AWS: Smithy UpdateHttpNamespace{Id (required), UpdaterRequestId, Namespace (required)} -> {OperationId}. Recorded as in cause 1.
Emulator: bare 32-character token.
Why/Fix: as cause 1, through the UpdateHttpNamespace handler's id allocation.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#5.

Cause 3 of 4: RegisterInstance's OperationId fails the recorded shape.
AWS: Smithy RegisterInstance{ServiceId (required), InstanceId (required), CreatorRequestId, Attributes (required)} -> {OperationId}. Recorded as in cause 1.
Emulator: bare 32-character token.
Why/Fix: as cause 1, through RegisterInstance's id allocation.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#11; same cluster also covers servicediscovery-instance-registration-error-recovery#task#5 and servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#6, #task#7, #task#8.

Cause 4 of 4: DeregisterInstance's OperationId fails the recorded shape.
AWS: Smithy DeregisterInstance{ServiceId (required), InstanceId (required)} -> {OperationId}. Recorded as in cause 1.
Emulator: bare 32-character token.
Why/Fix: as cause 1, through DeregisterInstance's id allocation.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#15; same cluster also covers servicediscovery-instance-registration-error-recovery#task#11 and servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#14, #task#16, #task#18.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. All four clusters are status-matched 200 responses whose OperationId member failed the corpus' shape/correlation assertion (the emulator values below are from this run and are NOT the requirement - the recorded FORM is):

Cause 1 - servicediscovery-dns-namespace-request-validation-errors#task#4, CreatePrivateDnsNamespace: '$.OperationId: value_mismatch (expected \'<shaped-not-asserted>\', got \'s9kay248x2ifq16ao9xyeaimsi61xgln\')'.
Cause 2 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#5, UpdateHttpNamespace: '$.OperationId: value_mismatch (expected \'{{update_http_namespace_op_id_3}}\', got \'4mivqu6e2y9e5x8jb2bsirsxg8c59zb3\')'.
Cause 3 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#11, RegisterInstance: '$.OperationId: value_mismatch (expected \'{{register_instance_op_id_4}}\', got \'8n4xw9blbm2q3wlj4srhez1nvc0zk7zk\')'. 5 steps affected.
Cause 4 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#15, DeregisterInstance: '$.OperationId: value_mismatch (expected \'{{deregister_instance_op_id_5}}\', got \'nzasx89jpzbtaibggme8r3o3shgq1y0c\')'. 5 steps affected.

In all four the HTTP status (200) and every other member matched; only the OperationId's form diverged. Note that the emulator's ids ARE internally consistent (the subsequent GetOperation polls resolve them), so this is a format divergence, not a broken correlation.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids; the OperationId assertion is the only failing check in each of those steps. With botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)) inspect the returned ids directly:
1. Cause 1: sd.create_private_dns_namespace(Name='<unique>.internal', Vpc='vpc-<id>')['OperationId'].
2. Cause 2: create an HTTP namespace, poll to SUCCESS, then sd.update_http_namespace(Id=ns, Namespace={'Description':'x'})['OperationId'].
3. Cause 3: sd.register_instance(ServiceId=svc, InstanceId='i1', Attributes={'AWS_INSTANCE_IPV4':'192.0.2.10'})['OperationId'].
4. Cause 4: sd.deregister_instance(ServiceId=svc, InstanceId='i1')['OperationId'].
Each returns a bare 32-character lowercase alphanumeric token; compare it with the operation-id form recorded in the fixtures above.
