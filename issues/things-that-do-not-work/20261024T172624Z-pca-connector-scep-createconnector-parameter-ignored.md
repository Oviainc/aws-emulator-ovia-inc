Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: fd06ba4dae3182271924f3bc530f4fe4ed0f438d

# VpcEndpointId has no observable effect: a VPC-endpoint connector reports the same public SCEP Endpoint as a public one

_Filed: 2026-10-24T17:26:24Z_
_Operation: CreateConnector | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `fd06ba4dae3182271924f3bc530f4fe4ed0f438d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `VpcEndpointId` changes how the connector is reachable. Its own documentation says "If you don't supply a value, by default Connector for SCEP creates a connector accessible over the public internet. If you provide a VPC endpoint ID, creates a connector accessible only through that specific VPC endpoint." The `Connector`/`ConnectorSummary` shapes carry no `VpcEndpointId` member, so the ONLY surface on which that choice is observable to a caller is `Connector$Endpoint` ("The connector's HTTPS public SCEP URL"): a connector restricted to one VPC endpoint must report a private, VPC-endpoint-qualified SCEP host, not the same public `amazonaws.com` host a public connector gets.

Actual: the handler stores `VpcEndpointId` on the record and then never uses it for anything. `crate::state::connector_endpoint(&region, &connector_id)` is called unconditionally with only the Region and the connector id, so `https://<uuid>.scep.pca-connector-scep.<region>.amazonaws.com` is produced byte-for-byte identically whether the create named a VPC endpoint or not. No operation returns the stored `VpcEndpointId`, and no response field differs, so the input is accepted (and validated against its @pattern/@length) but has zero observable effect anywhere in the emulated API: a caller cannot distinguish a private connector from a public one, and a workflow that configures SCEP clients against the reported Endpoint gets the public host for a connector that AWS would only serve through the VPC endpoint.

This is distinct from the open CreateConnector/wrong-behavior issue ("never produces a FAILED connector"), which asks for the referenced VPC endpoint to be RESOLVED so a bad id can settle the connector into FAILED/VPC_ENDPOINT_RESOURCE_NOT_FOUND. The gap here is that even for a perfectly good VpcEndpointId nothing in any response reflects it.

## Evidence

Smithy model: `CreateConnectorInput$VpcEndpointId` (len 13..22, pattern ^vpce-[0-9a-f]{8}([0-9a-f]{9})?$) doc = "If you don't supply a value, by default Connector for SCEP creates a connector accessible over the public internet. If you provide a VPC endpoint ID, creates a connector accessible only through that specific VPC endpoint."; `Connector$Endpoint` / `ConnectorSummary$Endpoint` doc = "The connector's HTTPS public SCEP URL" and neither shape models a `VpcEndpointId` member (smithy_query shape=Connector,ConnectorSummary), so Endpoint is the only place the access path can show up.

Handler: src/ops/create_connector.rs — `let endpoint = crate::state::connector_endpoint(&region, &connector_id);` is computed before and independently of `input.vpc_endpoint_id` (which is only moved into `StoredConnector.vpc_endpoint_id`); src/state.rs `connector_endpoint(region, connector_id)` = `format!("https://{connector_id}.scep.{ARN_SERVICE}.{region}.amazonaws.com")` — the stored `vpc_endpoint_id` field is read by nothing (`StoredConnector::to_connector()` / `to_summary()` do not surface it).

Test coverage: tests/all/connector_lifecycle.rs `created_connector_is_read_back_from_state` includes two cases that DO pass a VpcEndpointId ("vpce-0123abcd", "vpce-0123abcd456789abc"), but the only Endpoint assertion is the loose `endpoint.starts_with("https://") && endpoint.contains(id)` (around line 552) — no case asserts that the VPC-endpoint connector's Endpoint differs from the public one's, so the gap is uncovered.

## Steps to Reproduce

1. POST /connectors with {"CertificateAuthorityArn": "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555"} -> 202, note ConnectorArn A.
2. POST /connectors with the same CA ARN plus {"VpcEndpointId": "vpce-0123abcd456789abc"} -> 202, note ConnectorArn B.
3. Let both settle, then GET /connectors/{A} and GET /connectors/{B} and compare Connector.Endpoint: both are https://<connector-uuid>.scep.pca-connector-scep.<region>.amazonaws.com — the only difference is the uuid, so nothing in B's response records that it is reachable only through vpce-0123abcd456789abc.
4. GET /connectors (ListConnectors) and observe the same: ConnectorSummary for B carries no indication of the VPC endpoint either.
5. Expected: B's Endpoint reflects the private access path for the named VPC endpoint (and/or the VPC-endpoint choice is otherwise observable), rather than being indistinguishable from the public-internet connector A.
