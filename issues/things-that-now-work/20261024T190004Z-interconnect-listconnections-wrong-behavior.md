Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 38a5710b90801481080ff726caa37ba94cf84d5f

# Connection store is not region-scoped: a connection created through one region's endpoint is readable and listable through every other region

_Filed: 2026-10-24T19:00:04Z_
_Operation: ListConnections | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `38a5710b90801481080ff726caa37ba94cf84d5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Interconnect is a regional service (endpoint ruleset `https://interconnect.{Region}.{dnsSuffix}`, and ListEnvironments is documented as listing "all of the environments that can produce connections that will land in the called AWS region"). A Connection therefore belongs to the region it was created in: its ARN carries that region, and reads issued against a different region's endpoint do not see it.

The emulator keeps ALL connections in one unpartitioned map keyed only by `id` (`src/state.rs`: `pub connections: BTreeMap<String, StoredConnection>`), and the signing region is used for exactly one thing — minting the ARN at create time (`create_connection.rs` -> `crate::state::request_region(ctx)`). No read or write ever compares the request's region with the region the record was created in.

Observable divergence:
* Create a connection with a request signed for `eu-west-1` (ARN `arn:aws:interconnect:eu-west-1:...`), then call GetConnection / ListConnections signed for `us-east-1`: the emulator returns that connection — with its `eu-west-1` ARN — where the real service answers as if the connection does not exist in `us-east-1`.
* ListConnections signed for any region returns the union of all connections created in every region, so page counts, `nextToken` sequences and the `environmentId`/`provider`/`attachPoint` filters all operate over a cross-region set.
* The same leak applies to the connection-identifier operations (DeleteConnection, UpdateConnection, DescribeConnectionProposal via the shared `activation_keys` index) and to the ARN-resolving tagging operations, which match the stored ARN field-for-field and so are the only place where a region mismatch is noticed at all.

Expected: a connection is visible only to requests signed for the region it was created in; a read/update/delete of it from another region is refused the way an unknown identifier is. Actual: the store is global, so every region sees every connection.

## Evidence

Spec/semantics grounding: smithy_query service_info shows `endpointResolution.urlTemplates = https://interconnect.{Region}...` (regional endpoints, sigv4 signing name `interconnect`), and the ListEnvironments documentation scopes the service's catalog to "the called AWS region"; `Connection$arn` is `@pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`, i.e. every connection records a region. Region-scoped resource stores are the established pattern elsewhere in this repo (e.g. service-com-amazonaws-apprunner-apprunner `ServiceStore::live_in(region, arn)`, service-com-amazonaws-xray-awsxray `visible_groups(region)`, service-com-amazonaws-aiops-aiops `group(identifier, request_region)`).

Handler/state: services/service-com-amazonaws-interconnect-interconnect/src/state.rs — `ComAmazonawsInterconnectInterconnectState.connections` is a single `BTreeMap<String, StoredConnection>` with no region field on `StoredConnection`; `request_region(ctx)` exists but is called only from create_connection.rs (line ~166) to build the ARN. src/ops/get_connection.rs lines 144-162 and src/ops/list_connections.rs lines 159-186 look up / enumerate the whole map with `let _ = ctx;` — the request's region is not read at all.

Test coverage: the current tests assert the divergent behaviour. tests/all/connection_lifecycle.rs lines 498-643 creates each case with `call_in(case.region, "CreateConnection", ...)` (regions alternate per case, lines ~455), asserts the ARN carries that region (lines 523-531), then reads it back with the default-region harness `emulator.ok("GetConnection", ...)` (lines 636-643) and asserts the record is returned. Those rows would have to move to a per-region expectation when this is fixed. No existing issue covers region scoping of the connection store (the open tagging issues cover only the error CODE for a cross-region ARN; the wire-fidelity issue covers the environment catalog contents).

## Steps to Reproduce

1. Boot the emulator and sign a CreateConnection request for eu-west-1 (Credential=.../eu-west-1/interconnect/aws4_request) on a seeded environment; note the returned id and that its ARN is arn:aws:interconnect:eu-west-1:<account>:connection/<id>.
2. Call GetConnection with that identifier signed for us-east-1: expect the service to behave as it does for an unknown identifier in that region; observe HTTP 200 with the eu-west-1 record.
3. Call ListConnections signed for us-east-1: expect the eu-west-1 connection to be absent; observe it in `connections`, inflating the page and any filter/nextToken sequence.
4. Repeat with DeleteConnection / UpdateConnection signed for a third region (e.g. ap-southeast-2): observe both mutate the eu-west-1 connection.
