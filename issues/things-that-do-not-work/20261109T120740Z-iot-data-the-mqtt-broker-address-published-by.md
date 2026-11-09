Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.connected-client-connection-apis-reflect-live-session
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.connected-client-connection-apis-reflect-live-session|run#36|PublishedUrlRejected|client.connect(*ctx.endpoint_host(endpoint_host, 8883), keepalive=30)
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot-data: the MQTT broker address published by DescribeEndpoint is a real-AWS ATS host on 8883 that the emulator does not listen on, so no live client session can be established or observed

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: the address a client must use for an MQTT-over-TLS session is published as an AWS public hostname the emulator does not serve.
AWS: Smithy (service-com-amazonaws-iot-awsiotservice): DescribeEndpoint — GET /endpoint, output member endpointAddress : EndpointAddress. The address returned for the iot:Data-ATS endpoint type is the account's own data-plane host, and AWS serves MQTT over TLS on port 8883 and the HTTPS data-plane API on 443 at that host. A client authenticating with an X.509 certificate connects to exactly that host:8883; the connection-management reads (the connected-client / connection APIs) then reflect that live session.
Emulator: DescribeEndpoint answers 200 with a hostname of AWS's public ATS form, and the replay's attempt to use it as the MQTT host is refused: "endpoint_host was given <prefix>-ats.iot.<region>.amazonaws.com:8883 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address". No MQTT connect is attempted against the emulator, so the live-session state the connection APIs are supposed to reflect is never created.
Why: a device or test client that discovers its broker the documented way (DescribeEndpoint, then connect to host:8883) cannot connect to the emulator at all; session-dependent behavior — connected-client reads, retained-message delivery on connect, policy evaluation at CONNECT — is unreachable. The address is a value the emulator must own; an AWS-shaped name sends traffic off the emulator.
Fix: the iot DescribeEndpoint handler's endpoint-address construction, together with the iot-data/MQTT listener registration — the published host (and port, when the broker is not on 8883) must name the emulator's own broker listener.
Traces: iot-data.connected-client-connection-apis-reflect-live-session.

## Evidence
Expected: DescribeEndpoint(endpointType='iot:Data-ATS') -> 200 endpointAddress = a host the emulator serves; a TLS MQTT CONNECT to <that host>:8883 (or the emulator's own broker port) succeeds and the connection APIs report the live session. Actual: 200 with the AWS public form; replay raises simparity.published.PublishedUrlRejected — "endpoint_host was given <prefix>-ats.iot.<region>.amazonaws.com:8883 (published by the emulator or built by the check): its host is not one of the emulator's own hosts". Fixed text in the diagnosis is the rejection itself; the per-account prefix, region and port are values from the emulator's response, not requirements.

## Steps to Reproduce
1. Boot the emulator; call iot DescribeEndpoint with endpointType='iot:Data-ATS'.
2. Attempt an MQTT-over-TLS CONNECT to <endpointAddress>:8883 with a registered certificate — the host is not one the emulator listens on.
3. Replay iot-data.connected-client-connection-apis-reflect-live-session and observe the PublishedUrlRejected on endpoint_host before any CONNECT.
