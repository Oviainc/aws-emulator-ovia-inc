Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# UpdateRouterOutput changes destinationTransitEncryption on a connected output, the precondition the sibling UpdateRouterInput enforces for sourceTransitDecryption

_Filed: 2026-10-28T21:02:57Z_
_Operation: UpdateRouterOutput | Class: validation_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both router output configuration variants that connect to another AWS resource document the same precondition: the transit encryption may not be changed while the output is connected. UpdateRouterOutput never looks at it. The handler resolves the replacement configuration's references, refuses a configuration-TYPE change, and refuses a `FabricConfiguration` change while the output is routed — but it never compares the requested `destinationTransitEncryption` against the stored one, so a request that replaces a MEDIACONNECT_FLOW (or MEDIALIVE_INPUT) configuration while `flowArn`/`flowSourceArn` (or `mediaLiveInputArn`/`mediaLivePipelineId`) are still set is accepted (202) and the new encryption is stored.

The crate already implements exactly this rule for the INPUT side: UpdateRouterInput refuses a replacement whose `sourceTransitDecryption` differs while `router_input_connected_source` reports a connection, through two helpers in `crate::state` whose doc quotes the model note. There is no router-output counterpart of either helper (`grep` for `connected`, `transit_encryption` in `src/ops/update_router_output.rs` returns nothing), so the identically documented half of the same rule is simply absent on the output side.

## Evidence

Model (smithy_query query="shape" shape="MediaConnectFlowRouterOutputConfiguration"), doc on the shape: "To connect the router output to the flow source, provide both flowArn and flowSourceArn. To disconnect it, call the update operation and omit both fields while keeping this configuration object. NOTE: You can't change destinationTransitEncryption while the router output is connected to a flow source. To change it, first disconnect the router output, then update the encryption." The same note on MediaLiveInputRouterOutputConfiguration: "You can't change destinationTransitEncryption while the router output is connected to a MediaLive input. To change it, first disconnect the router output, then update the encryption."

The handler's only state-dependent refusal — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_router_output.rs:307-327:
```
        // "You cannot update the fabric configuration while the output has an active route.
        // You must unroute the output before updating the fabric configuration"
        // (`UpdateRouterOutput$FabricConfiguration`) ...
        if input.fabric_configuration.is_some() && stored_is_routed {
```
and the unconditional write of the replacement configuration — update_router_output.rs:371-373:
```
        if let Some(configuration) = requested_configuration {
            output.configuration = configuration;
        }
```
`grep -n 'connected|transit_encryption|TransitEncryption' src/ops/update_router_output.rs` → "No matches."

The sibling that DOES enforce the twin rule — src/ops/update_router_input.rs:292-315:
```
            // "You can't change sourceTransitDecryption while the router input is connected
            // to a flow output. To change it, first disconnect the router input, then update
            // the decryption" ... which is what `ConflictException` describes.
            if let Some(source) =
                crate::state::router_input_connected_source(&stored_configuration)
            {
                if crate::state::router_input_source_transit_decryption_differs(
                    &stored_configuration,
                    configuration,
                ) {
                    return Err(conflict(format!(
                        "Router input {stored_arn} is connected to {source}, so its \
                         SourceTransitDecryption cannot be changed. Disconnect the router \
                         input first, then update the decryption."
```
backed by src/state.rs:3824-3834:
```
/// Whether the configuration `requested` asks for a different SOURCE TRANSIT DECRYPTION
/// than the one `stored` carries.
/// "You can't change `sourceTransitDecryption` while the router input is connected to a
/// flow output. ...", so an operation that replaces a configuration asks HERE whether it
/// would change that member.
pub fn router_input_source_transit_decryption_differs(
```

Test coverage: the input-side rule is pinned by tests/all/update_router_input.rs:958-1013 ("You can't change sourceTransitDecryption while the router input is connected."). The output-side refusal table, tests/all/update_router_output.rs:613-700, covers only a not-found ARN, two configuration-type changes, a contradicting Protocol, bitrate and tier — no encryption case. The one update case that repoints a connected MEDIACONNECT_FLOW output (tests/all/update_router_output.rs:172-177) keeps the same `{"encryptionKeyType":"AUTOMATIC"}` encryption, so adding the missing refusal leaves every existing test passing.

## Steps to Reproduce

1. POST /v1/routerNetworkInterface and POST /v1/flows to mint an interface, a flow and a flow source.
2. POST /v1/routerOutput with configuration {"MediaConnectFlow":{"flowArn":FLOW,"flowSourceArn":SOURCE,"destinationTransitEncryption":{"encryptionKeyType":"AUTOMATIC","encryptionKeyConfiguration":{"Automatic":{}}}}} — 201; the output is CONNECTED (both members set).
3. PUT /v1/routerOutput/{Arn} with the same flowArn/flowSourceArn but destinationTransitEncryption {"encryptionKeyType":"SECRETS_MANAGER","encryptionKeyConfiguration":{"SecretsManager":{"secretArn":...,"roleArn":...}}}.
4. Observe 202 and the stored output reporting the new SECRETS_MANAGER encryption (GET /v1/routerOutputs), although the model says the member cannot be changed while the output is connected — the UpdateRouterInput path refuses the identical request shape with 409 ConflictException.
