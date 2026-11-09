Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-account-setup-custom-endpoint
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-account-setup-custom-endpoint
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|RegisterCustomEndpoint|2xx->2xx|$.EndpointAddress
Issue-Key: iot-managed-integrations|GetCustomEndpoint|2xx->2xx|$.EndpointAddress
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot-managed-integrations custom endpoint announces an unreachable amazonaws.com address instead of a host the emulator serves

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCustomEndpoint

## Description
The account's dedicated IoT managed integrations custom endpoint is published as a real AWS public DNS name (`<label>.api.iotmanagedintegrations.<region>.amazonaws.com`). Nothing in the emulator serves or resolves that host, so the address it hands a client is not an address the emulator can be reached on. Both operations that report the singleton endpoint are affected, from the one address-synthesis helper, so they are bundled as one fault with two causes.

Cause 1 of 2: RegisterCustomEndpoint (fixture iot-managed-integrations-account-setup-custom-endpoint, step task#1).
  AWS: Smithy: RegisterCustomEndpoint -> RegisterCustomEndpointResponse.EndpointAddress, target EndpointAddress, @required, @length min 1 max 256, @pattern ^[A-Za-z0-9._@-]+$; http POST /custom-endpoint, success code 201. The modelled documentation says EndpointAddress is "The IoT managed integrations dedicated, custom endpoint for the device to route traffic through" — i.e. a host a device actually connects to. On AWS that host is served by the account's regional endpoint. Recorded behavior shows status 201 with a single EndpointAddress member; the recording's own host value is an account-specific per-run value and is not the requirement.
  Emulator: answers 201 with EndpointAddress = `25813800000000.api.iotmanagedintegrations.eu-west-1.amazonaws.com`, a public AWS DNS name that the emulator neither serves nor is reached on. The replay verdict is `host_not_own` on $.EndpointAddress: the emulator announced an address other than the one it is reached on.
  Why: a client that does what the operation exists for — take the returned EndpointAddress and route device traffic to it — leaves the emulator entirely. The connection either fails to resolve/connect or, worse, is directed at real AWS infrastructure. Every downstream flow built on the custom endpoint (device onboarding, traffic routing, trust configuration) is unexercisable, and the only way a caller can keep working is to ignore the value the API just gave it.
  Fix: the endpoint-address synthesis in the service's state layer, used by the RegisterCustomEndpoint handler. The address must be built from the host/authority the emulator is actually serving on (the host the request arrived at, plus the emulator's own data listener), keeping the dedicated per-account label as the leading label so the value still satisfies the modelled @length 1..256 and @pattern ^[A-Za-z0-9._@-]+$ and stays stable across repeat registers and reads. It must not be hardcoded to the `amazonaws.com` production DNS suffix.

Cause 2 of 2: GetCustomEndpoint (fixture iot-managed-integrations-account-setup-custom-endpoint, step task#2).
  AWS: Smithy: GetCustomEndpoint -> GetCustomEndpointResponse.EndpointAddress, target EndpointAddress, @required, @length min 1 max 256, @pattern ^[A-Za-z0-9._@-]+$; http GET /custom-endpoint, success code 200, @readonly; errors include ResourceNotFoundException (404) when no endpoint has been registered. The read reports the same singleton address the register minted, so whatever the register publishes, the read must publish identically — and must likewise be reachable.
  Emulator: answers 200 with the same stored `...api.iotmanagedintegrations.eu-west-1.amazonaws.com` value, again flagged `host_not_own` on $.EndpointAddress. The read is consistent with the register, which is correct, but it propagates the same unreachable host because the stored value itself is the production DNS name.
  Why: a client that discovers the endpoint through the read path (rather than re-registering) gets the same unreachable address, so the defect cannot be worked around by reading instead of registering.
  Fix: the same address synthesis — because the address is minted once and stored, correcting it at mint time (and/or rendering it against the serving host on read) fixes both the register and the read together; GetCustomEndpoint's own handler needs no separate rule beyond continuing to echo the stored singleton.

Note: no existing open iot-managed-integrations issue covers the custom endpoint address; the open issues for this service concern CreateAccountAssociation/connector-destination resolution, several refusal-message bodies (DeleteOtaTaskConfiguration, UpdateCloudConnector, GetNotificationConfiguration, DeleteCloudConnector) and the UpdateManagedThing 204 Content-Length header — none of them touch RegisterCustomEndpoint or GetCustomEndpoint.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200, data listener accepting):

Cause 1 — fixture iot-managed-integrations-account-setup-custom-endpoint, step task#1, operation RegisterCustomEndpoint, verdict mismatch:
  expected status 201, actual status 201
  detail: $.EndpointAddress: host_not_own (expected '{{endpoint_address}}', got '25813800000000.api.iotmanagedintegrations.eu-west-1.amazonaws.com')
  divergence_key: service=iot-managed-integrations|RegisterCustomEndpoint|status=201->201|keys=$.EndpointAddress

Cause 2 — fixture iot-managed-integrations-account-setup-custom-endpoint, step task#2, operation GetCustomEndpoint, verdict mismatch:
  expected status 200, actual status 200
  detail: $.EndpointAddress: host_not_own (expected '25813800000000.api.iotmanagedintegrations.eu-west-1.amazonaws.com', got '25813800000000.api.iotmanagedintegrations.eu-west-1.amazonaws.com')
  divergence_key: service=iot-managed-integrations|GetCustomEndpoint|status=200->200|keys=$.EndpointAddress

In both clusters the HTTP status and the response shape match; the sole divergence is that the announced host is not one the emulator serves. The specific label ('25813800000000'), region and account are per-run values and are not the requirement — the requirement is that the announced authority is the emulator's own.

Source corroboration (citations, not the requirement): services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs:955-957 builds the address as `format!("{label}.{SERVICE_ENDPOINT_PREFIX}.{region}.{AWS_DNS_SUFFIX}")`; it is minted and stored once in `register_custom_endpoint` at state.rs:1259-1271 and read back unchanged by `custom_endpoint_in` at state.rs:1241-1243. The handler at src/ops/register_custom_endpoint.rs:199-214 returns that stored value verbatim.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture iot-managed-integrations-account-setup-custom-endpoint step task#1 — POST /custom-endpoint (RegisterCustomEndpoint), signed for eu-west-1, with no body members. Observe 201 whose EndpointAddress ends in `.api.iotmanagedintegrations.eu-west-1.amazonaws.com`, a host the emulator does not serve; the step fails the host_not_own check on $.EndpointAddress.
3. Cause 2: replay the same fixture's step task#2 — GET /custom-endpoint (GetCustomEndpoint) in the same region. Observe 200 echoing the same production-DNS address, failing host_not_own on $.EndpointAddress.
4. Confirm the practical consequence: attempt to open a connection to the returned EndpointAddress. It is not an authority the emulator answers on, whereas the emulator's own data listener does answer.
5. After the fix, re-replay both steps (iot-managed-integrations-account-setup-custom-endpoint#task#1 and #task#2); both must report an EndpointAddress whose authority is the emulator's own serving host, identical between the register and the read, and still matching @length 1..256 and @pattern ^[A-Za-z0-9._@-]+$.
