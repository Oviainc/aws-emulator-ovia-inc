Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# A router input Configuration is stored without checking its own content: a STANDARD Protocol that contradicts its ProtocolConfiguration, and FAILOVER/MERGE ProtocolConfigurations that are not exactly two of the same protocol type

_Filed: 2026-10-28T21:02:57Z_
_Operation: CreateRouterInput | Class: validation_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateRouterInput stores `input.configuration` verbatim (UpdateRouterInput writes a replacement the same way). Only the ARNs the configuration REFERENCES are resolved; nothing checks the configuration's own content, so two malformed configurations are persisted and reported back:

1. A STANDARD configuration carries both a required `ProtocolConfiguration` (whose union variant IS the protocol) and an optional `Protocol` enum. A request naming `protocol: "RIST"` with `protocolConfiguration: {"rtp": {...}}` is stored self-contradicting, and GetRouterInput/ListRouterInputs report it. The router OUTPUT side of this crate carries exactly the same member pair and refuses that request through `crate::state::normalized_router_output_configuration` — which also DERIVES the stored `Protocol` from the configuration, so an output that named none still reports one while an input that named none reports nothing.

2. FAILOVER and MERGE configurations document `ProtocolConfigurations` as "a list of exactly two protocol configurations ... Both must use the same protocol type", and neither list shape carries `@length`, so the boundary walk enforces nothing. A FAILOVER input created with zero, one or five entries — or with one `rtp` and one `srtCaller` entry — is accepted. The crate then reports stream details for source index 0 AND 1 for such an input, which its own comment justifies by "the index is the position the configuration lists the source at" — an index that names no entry at all for a one-element list.

## Evidence

Model (smithy_query query="shape" shape="FailoverRouterInputConfiguration"), member ProtocolConfigurations: "A list of exactly two protocol configurations for the failover input sources. Both must use the same protocol type." The same sentence on MergeRouterInputConfiguration$ProtocolConfigurations ("...for the merge input sources. Both must use the same protocol type."). Their list shapes carry no length constraint: smithy_query query="shape" shape="FailoverRouterInputProtocolConfigurationList" → {"type": "list", "member": "FailoverRouterInputProtocolConfiguration"} (no `length`). `grep -rn protocol_configurations src/` matches ONLY the two struct fields in src/ops/types/shared_types.rs:927 and :954 — no handler or state function ever inspects the list.

StandardRouterInputConfiguration (same query): `ProtocolConfiguration` "required": true and `Protocol` an optional enum, "The protocol used by the standard router input" — the identical pair StandardRouterOutputConfiguration carries.

The create stores the configuration untouched — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/create_router_input.rs:358 and :377:
```
        let configuration = input.configuration.clone();
...
                configuration,
```
and the update writes a replacement the same way — src/ops/update_router_input.rs:366-368:
```
        if let Some(configuration) = input.configuration.clone() {
            router_input.configuration = configuration;
        }
```
The sibling rule the output side owns — src/state.rs:4159-4196:
```
/// A STANDARD output's `Protocol` is implied by the variant of its `ProtocolConfiguration`
/// ([`router_output_protocol`]), so the stored configuration reports the protocol that
/// configuration configures, and a request whose `Protocol` names a different one is
/// refused rather than stored self-contradicting. ... Every operation that takes a
/// configuration (CreateRouterOutput, UpdateRouterOutput) normalises it HERE
pub fn normalized_router_output_configuration(
...
                if named != configured {
                    return Err(format!(
                        "Configuration sets a {configured} protocol configuration, but \
                         Protocol names {named}."
```
called from src/ops/create_router_output.rs:241 and src/ops/update_router_output.rs:274; `grep` finds no `normalized_router_input_configuration` anywhere.

The crate's own assumption that a failover/merge configuration lists exactly two sources — src/state.rs:3653-3656 and :3666-3689:
```
/// ... a failover and a
/// merge input still report both of their indexed entries, because the model makes those
/// two members required, and the index is the position the configuration lists the source
/// at.
...
                source_index_zero_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 0,
...
                source_index_one_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 1,
```

Test coverage: every router input test sends a `protocol` that agrees with its `protocolConfiguration` (tests/all/router_input_restart.rs:165-199, tests/all/router_input_thumbnail.rs:354) and none sends a failover/merge list of a size other than two, so no test pins the current acceptance; the output side's contradiction case is pinned at tests/all/update_router_output.rs:644-655 ("a Protocol that contradicts the protocol configuration" → 400).

## Steps to Reproduce

1. POST /v1/routerNetworkInterface to mint an interface ARN.
2. POST /v1/routerInput with configuration {"Standard":{"networkInterfaceArn":IFACE,"protocol":"RIST","protocolConfiguration":{"Rtp":{"port":5000}}}} — observe 201 and GET /v1/routerInputs?arns=... reporting configuration.standard.protocol == "RIST" beside an rtp protocolConfiguration (the same contradiction is 400 BadRequestException on PUT/POST /v1/routerOutput).
3. POST /v1/routerInput with configuration {"Failover":{"networkInterfaceArn":IFACE,"sourcePriorityMode":...,"protocolConfigurations":[{"Rtp":{"port":5000}}]}} (ONE entry) — observe 201 instead of a refusal, although the member is documented as "a list of exactly two protocol configurations".
4. GET /v1/routerInputs?arns=<that ARN> — observe streamDetails reporting both sourceIndexZeroStreamDetails and sourceIndexOneStreamDetails for a configuration that lists one source.
5. Repeat step 3 with two entries of DIFFERENT variants ({"Rtp":...} and {"SrtCaller":...}) — observe 201, although "Both must use the same protocol type".
