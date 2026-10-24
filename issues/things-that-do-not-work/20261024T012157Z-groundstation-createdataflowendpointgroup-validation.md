Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CreateDataflowEndpointGroup accepts an unusable socket port and un-named/duplicate endpoint names

_Filed: 2026-10-24T01:21:57Z_
_Operation: CreateDataflowEndpointGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Beyond the endpoint-kind checks, the handler validates nothing about the endpoint bodies it stores. Two classes of request AWS does not accept are committed to state and reported back by GetDataflowEndpointGroup:

(a) `SocketAddress.port` is a plain modeled `Integer` with no range, so the boundary validator lets `0`, a negative number, or a value above 65535 through, and the handler stores it. A dataflow endpoint whose address is not a bindable TCP/UDP port is not something Ground Station can connect a stream to; AWS refuses it with the operation's modeled `InvalidParameterException` (HTTP 431).

(b) The `name` of a `DataflowEndpoint` is how a mission profile's `dataflowEndpointConfig.dataflowEndpointName` selects an endpoint inside the group — the operation's own documentation says so ("The `name` field in each endpoint is used in your mission profile `DataflowEndpointConfig` to specify which endpoints to use during a contact... each `Config` must match a `DataflowEndpoint` in the same group"). The handler accepts a group whose endpoints carry no name at all, and a group holding two endpoints with the SAME name, which makes that match ambiguous/impossible; AWS refuses both with `InvalidParameterException`.

Expected: both refused with 431 and nothing stored. Actual: HTTP 200, the group is created, and the unusable endpoints come back verbatim from GetDataflowEndpointGroup/ListDataflowEndpointGroups.

## Evidence

Spec: `smithy_query(query="shape", shape="SocketAddress")` — `name` and `port` are required but `port` carries NO `range`, so nothing upstream of the handler bounds it; `smithy_query(query="doc", shape="CreateDataflowEndpointGroup")` — "The `name` field in each endpoint is used in your mission profile `DataflowEndpointConfig` to specify which endpoints to use during a contact. When a contact uses multiple `DataflowEndpointConfig` objects, each `Config` must match a `DataflowEndpoint` in the same group", which only holds if a name is present and unique within the group; the operation declares `InvalidParameterException` (431).
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_dataflow_endpoint_group.rs` lines ~180-236 — the loop checks the number of endpoint members, the presence of `securityDetails` and kind consistency, then calls `endpoint_as_created` which only rewrites `status`/`healthStatus`/`healthReasons`; no inspection of `endpoint.address.port` and no inspection of `endpoint.name` (not for presence, not for collisions).
Tests: `tests/all/dataflow_endpoint_groups.rs` builds every case from the one `security_details()`/endpoint body helper with a fixed valid port and a single name, and asserts round-trip, health overriding, kind mixing and the pre/post-pass bounds; nothing asserts anything about port values or name uniqueness, so adding the rules breaks no existing assertion.

## Steps to Reproduce

1. POST /dataflowEndpointGroup with endpointDetails=[{"endpoint": {"name": "dfe-1", "address": {"name": "10.0.0.1", "port": 0}, "mtu": 1500}, "securityDetails": {...}}] — observe 200 where AWS refuses port 0 with InvalidParameterException (431); repeat with port -1 and port 99999, same 200.
2. GET /dataflowEndpointGroup/{id} — the stored endpoint reports the unusable port back.
3. POST /dataflowEndpointGroup with two `endpoint` items that both set "name": "dfe-dup" (valid addresses, securityDetails present) — observe 200, where AWS refuses the duplicate name because a mission profile's dataflowEndpointName could not select between them.
4. POST /dataflowEndpointGroup with one `endpoint` item that omits "name" entirely — observe 200, where AWS refuses an endpoint no dataflowEndpointConfig can reference.
