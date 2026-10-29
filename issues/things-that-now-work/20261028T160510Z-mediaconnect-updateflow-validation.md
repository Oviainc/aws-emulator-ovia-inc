Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# UpdateFlow lets a flow holding an NDI output be downgraded to a flow size that cannot carry NDI

_Filed: 2026-10-28T16:05:10Z_
_Operation: UpdateFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateFlow guards the NDI-needs-the-larger-flow rule by looking at the merged `NdiConfig` and, when the request moves `FlowSize`, at the flow's stored SOURCES only. The flow's stored OUTPUTS are never consulted, so a flow created LARGE with an output on the NDI transport (`protocol: ndi-speed-hq`) and no `NdiConfig` is happily downgraded to MEDIUM.

That leaves a stored flow in a shape the crate's own sibling paths refuse to create: CreateFlow refuses an NDI output on a non-NDI-capable size (its `requested_ndi_feature` reads `output_request_uses_ndi`), AddFlowOutputs refuses appending one, and UpdateFlowOutput refuses moving an existing output onto the NDI transport. After the downgrade the flow holds an NDI output on a MEDIUM flow, and adding a second identical output is refused — two answers for one state.

The third NDI marker (an output's protocol) is the one marker UpdateFlow drops; the source marker and the `NdiConfig` marker are both checked.

## Evidence

UpdateFlow reads only sources when the size moves — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_flow.rs:325-343:
```
        if input.ndi_config.is_some() || input.flow_size.is_some() {
            // What uses NDI on the flow this request leaves behind, worded for the refusal
            // `crate::state::ndi_flow_size_rejection` states — the same markers CreateFlow
            // and the add paths read.
            let ndi_feature = if crate::state::ndi_is_enabled(ndi_config.as_ref()) {
                ...
            } else if input.flow_size.is_some() {
                flow.sources
                    .iter()
                    .find(|source| crate::state::stored_source_uses_ndi(source))
                    .map(|source| format!("The NDI source {:?}", source.name))
```
The sibling create path DOES read the output marker — src/ops/create_flow.rs:166-177:
```
    if let Some((index, output)) = input
        .outputs
        .iter()
        .flatten()
        .enumerate()
        .find(|(_, output)| crate::state::output_request_uses_ndi(output))
    {
        return Some(match output.name.as_deref() {
            Some(name) => format!("The NDI output {name:?}"),
```
and so does the add path, which states that UpdateFlow reads the same rule — src/ops/add_flow_outputs.rs:240-256:
```
        // carry NDI — the rule, its markers and its wording all read from the one place
        // CreateFlow and UpdateFlow read them from (`crate::state`).
        if let Some((offset, output)) = input
            .outputs
            .iter()
            .enumerate()
            .find(|(_, output)| crate::state::output_request_uses_ndi(output))
        ...
            if let Some(rejection) =
                crate::state::ndi_flow_size_rejection(&feature, flow.flow_size.as_deref())
```
The shared rule's own doc says it is the one place every such path words the refusal — src/state.rs:1076-1084:
```
/// Every operation that can put NDI on a flow words the refusal HERE, so the create side,
/// the add sides and the update sides state one rule for one reason.
...
         one that supports NDI before enabling NDI sources or outputs on it.
```
Test coverage: tests/all/update_flow.rs:671-737 (`a_flow_with_an_ndi_source_keeps_an_ndi_capable_size`) pins the SOURCE case only; tests/all/flow_ndi_sizing.rs:463 and :538 pin the add/update-output cases on a fixed size. No test exercises a size downgrade on a flow holding an NDI output, so nothing pins today's acceptance.

## Steps to Reproduce

1. POST /v1/flows with {"name":"f","flowSize":"LARGE","source":{"name":"src","protocol":"rtp","whitelistCidr":"10.0.0.0/16"},"outputs":[{"name":"ndi-out","protocol":"ndi-speed-hq"}]} — 201 (the NDI output is accepted because the size carries NDI).
2. PUT /v1/flows/{FlowArn} with {"flowSize":"MEDIUM"} — observe 202 and the stored flow now MEDIUM while it still holds the `ndi-speed-hq` output.
3. POST /v1/flows/{FlowArn}/outputs with [{"name":"ndi-out-2","protocol":"ndi-speed-hq"}] — observe 400 BadRequestException "… needs a flow of size LARGE or LARGE_4X; this flow's size is MEDIUM", i.e. the state step 2 produced is one the add path refuses to create.
4. Compare: a flow whose SOURCE is NDI refuses the same downgrade with 400 (tests/all/update_flow.rs:693-699).
