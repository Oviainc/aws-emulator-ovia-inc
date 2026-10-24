Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Connector store is not partitioned by Region: a connector created through one Region's endpoint is listed and resolvable through every other

_Filed: 2026-10-24T05:24:08Z_
_Operation: ListConnectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: Connector for SCEP is a regional service. A connector created by a request signed for eu-west-2 exists only in eu-west-2 — a ListConnectors call signed for us-east-1 must not return it, and GetConnector / ListChallengeMetadata / GetChallengePassword / DeleteConnector addressed at us-east-1 with that eu-west-2 ARN must answer ResourceNotFoundException (404) (the ARN's Region segment names the only Region in which the resource exists).

Actual: the emulator keeps ONE global connector map keyed solely by ARN and never consults the Region the request was signed for when reading or listing. The request Region is used only to MINT the ARN in CreateConnector; every reader then resolves or lists across all Regions:
- ListConnectors returns every stored connector, whatever Region its ARN names, so one Region's endpoint reports another Region's inventory (and a paginated walk interleaves them).
- GetConnector, ListChallengeMetadata, GetChallengePassword and DeleteConnector succeed on a foreign-Region ARN instead of 404.
A client that creates in one Region and lists in another therefore sees resources real AWS would not show, and a cross-Region typo that AWS reports as not-found silently succeeds.

## Evidence

Spec/well-known semantics: the service is regional (aws.api#service endpointPrefix/arnNamespace `pca-connector-scep`, sigv4 signing per Region; OPERATIONS.md wire protocol section) and every resource ARN embeds its Region (ConnectorArn pattern `^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector/<uuid>$`, smithy_query shape=ConnectorArn). Region isolation of per-account resources is well-established AWS behavior, and the same gap has been accepted as a fidelity bug elsewhere in this repo (e.g. keyspaces ListKeyspaces, MediaPackage DescribeChannel, iotsecuretunneling DescribeTunnel).

Handler/state: src/state.rs line ~330 — `pub connectors: BTreeMap<String, StoredConnector>` is the whole store, keyed by ARN with no Region dimension; `settle`/`challenge`/`remove_challenge` likewise key on ARN only. src/ops/list_connectors.rs lines 184-194 — iterates `store.connectors` in full and pages it; `ctx` is discarded (`let _ = ctx;` line 161), so `crate::state::request_region(ctx)` is never called. src/ops/get_connector.rs lines 154-165, src/ops/list_challenge_metadata.rs lines 168-195, src/ops/get_challenge_password.rs lines 151-161, src/ops/delete_connector.rs lines 179-196 — all discard `ctx` and look the ARN up directly. src/ops/create_connector.rs lines 192 and 272-274 — the request Region is read, but only to build the ARN and the endpoint.

Test coverage: none. tests/all/connector_lifecycle.rs and tests/all/connector_deletion.rs drive every call with the single REGION constant `eu-west-2` (connector_lifecycle.rs line 25); no case signs a second Region, so cross-Region visibility is unasserted in either direction.

## Steps to Reproduce

1. Send CreateConnector signed for eu-west-2 (Authorization credential scope .../eu-west-2/pca-connector-scep/aws4_request) with a valid CertificateAuthorityArn; note the returned ConnectorArn, which carries `eu-west-2`.
2. Send ListConnectors signed for us-east-1. Actual: the eu-west-2 connector is returned in `Connectors`. Expected: an empty `Connectors` list for a Region in which nothing was created.
3. Send GetConnector signed for us-east-1 with the eu-west-2 ConnectorArn. Actual: HTTP 200 with the full connector. Expected: HTTP 404 ResourceNotFoundException.
4. Create a challenge on the eu-west-2 connector, then call GetChallengePassword / ListChallengeMetadata / DeleteConnector signed for us-east-1 with those ARNs. Actual: they succeed (and the delete tears the other Region's connector down). Expected: 404 ResourceNotFoundException.
