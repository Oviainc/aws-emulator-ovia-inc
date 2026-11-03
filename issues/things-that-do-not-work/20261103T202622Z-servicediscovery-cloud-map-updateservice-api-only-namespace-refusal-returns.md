Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-service-attributes-crud-and-limits
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-service-attributes-crud-and-limits
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|UpdateService|4xx->4xx|$.Message
Issue-Commit: 7395066f6d0f65e771980acfc36b289985512441

# Cloud Map UpdateService API-only-namespace refusal returns emulator prose, not AWS's InvalidInput message

> _Note: this issue was identified against the code base at commit `7395066f6d0f65e771980acfc36b289985512441`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
One net-new wire divergence in the servicediscovery (Cloud Map) emulator: UpdateService correctly refuses a service that lives in an HTTP (API-only) namespace with HTTP 400 InvalidInput, but the Message body is emulator-authored prose instead of AWS's fixed refusal text. An HTTP status, an error code and an error message are all requirements, so the message alone is the defect here.

Cause 1 of 1: UpdateService on a service in an API-only (HTTP) namespace carries the wrong InvalidInput Message.
AWS: Smithy (services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json) UpdateService{Id: Arn (required, @length 0..255), Service: ServiceChange (required, members Description/DnsConfig/HealthCheckConfig)} -> {OperationId}; errors InvalidInput (400), ServiceNotFound (404), DuplicateRequest (409). InvalidInput is documented as 'One or more specified values aren't valid ...' and carries a single Message member. Recorded response: HTTP 400, error code InvalidInput, Message 'Service in API-only namespace cannot be updated.' - that message is entirely FIXED text: it has no variable parts at all, naming neither the service id, the namespace id nor the operation.
Emulator: HTTP 400 with the right error code but Message "The service <serviceId> is associated with an HTTP namespace, so its settings can't be updated with UpdateService." - a different template whose variable parts are the service id from the request and the operation name.
Why: a client or test that asserts on AWS's documented refusal text (or matches it to distinguish this refusal from other InvalidInput causes on the same call) diverges; and the service id the emulator embeds is per-run state, so the emulator's message is also not stable text a caller can match on.
Fix: the API-only-namespace refusal construction in UpdateService's handler - it must emit the fixed string above with no interpolation.
Traces: servicediscovery-service-attributes-crud-and-limits#task#4.

Related (does NOT cover this): the open issue 20261029T203804Z-servicediscovery-cloud-map-400-responses-carry-wrong-or-absent.md discusses this refusal in prose, but its replay pointer carries only the CreatePublicDnsNamespace, ListNamespaces, TagResource and UpdateServiceAttributes Message keys - the UpdateService key is absent from it, so this step is not exercised by that issue's replay and the replay report reports it unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 183 steps, 130 passed, 53 mismatched, score 71.0%, 27 divergence clusters.

servicediscovery-service-attributes-crud-and-limits#task#4, UpdateService, expected 400 / actual 400 (status and error code match): '$.Message: value_mismatch (expected \'Service in API-only namespace cannot be updated.\', got "The service srv-gttfihsjieqmddum is associated with an HTTP namespace, so its settings can\'t be updated with UpdateService.")'. Divergence key servicediscovery|UpdateService|status=400->400|keys=$.Message. owned_by: none. The srv-... id in the actual message is this run's generated service id, not a requirement - the requirement is that the whole expected message is fixed text with no interpolated id.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture step in Issue-Trace/Issue-Step, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_http_namespace(Name='<unique>')['OperationId']; poll sd.get_operation(OperationId=op) to SUCCESS and take ns = Operation['Targets']['NAMESPACE'].
2. svc = sd.create_service(Name='<unique>', NamespaceId=ns)['Service']['Id'].
3. try: sd.update_service(Id=svc, Service={'Description': 'x', 'DnsConfig': {'DnsRecords': [{'Type': 'A', 'TTL': 60}]}}) -> expect ClientError; read e.response['Error']['Code'] == 'InvalidInput' (matches) and e.response['Error']['Message'], which must equal exactly 'Service in API-only namespace cannot be updated.' The emulator instead returns the HTTP-namespace prose naming the service id and the operation.
