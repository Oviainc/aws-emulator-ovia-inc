Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# DeleteConnector answers 202 for a connector ARN that holds no record instead of ResourceNotFoundException

_Filed: 2026-10-24T02:08:31Z_
_Operation: DeleteConnector | Class: error-response_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeleteConnector against a well-formed connector ARN that names no stored connector fails with ResourceNotFoundException (HTTP 404, x-amzn-errortype: ResourceNotFoundException, with ResourceId/ResourceType members). That modeled error is reachable for this operation ONLY through a missing connector, because the operation's whole input is the connector ARN.

Actual: the handler treats @idempotent as "suppress 404" and unconditionally answers 202 with an empty body, whether or not a record existed. `store.connectors.remove(&input.connector_arn)` discards the removal result and the handler returns `Ok(DeleteConnectorOutput)`. As a consequence ResourceNotFoundException is dead code for DeleteConnector in this emulator, and a client that uses the 404 to distinguish "already deleted / never existed" from "deletion accepted" cannot do so.

Note @idempotent in Smithy means the request may be safely retried, not that a delete of an absent resource must succeed; AWS private-CA-family deletes (e.g. acm-pca DeleteCertificateAuthority, pca-connector-* DeleteConnector) report ResourceNotFoundException for an ARN with no record.

## Evidence

Smithy model (smithy_query operation=DeleteConnector): errors include ResourceNotFoundException (errorKind=client, httpError=404), whose doc is "The operation tried to access a nonexistent resource."; input is only `ConnectorArn` (@httpLabel, @required). Handler: services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/delete_connector.rs lines 200-204 — comment "a connector ARN that holds no record is a 202 rather than a 404" followed by `store.connectors.remove(...)` with the boolean dropped. Tests ENTRENCH the current behavior rather than the spec: tests/all/connector_lifecycle.rs `deletes_are_observable_and_idempotent` asserts 202 on the repeat delete (lines 1240-1246); no test asserts 404 for DeleteConnector.

## Steps to Reproduce

1. Start the emulator and call CreateConnector, then DeleteConnector once the connector is ACTIVE (202).
2. Call DeleteConnector a second time with the same ConnectorArn.
3. Observe HTTP 202 with an empty body; expected HTTP 404 ResourceNotFoundException with ResourceId=<connector ARN>, ResourceType=Connector.
4. Equivalently, call DeleteConnector with a pattern-valid connector ARN that was never created and observe 202 instead of 404.
