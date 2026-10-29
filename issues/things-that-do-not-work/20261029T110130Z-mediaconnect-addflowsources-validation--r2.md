Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1ff82dd4da31f037aed5d48a06cbab4eda93dc7c

# A ground-to-cloud bridge is given more flow outputs than its own IngressGatewayBridge.MaxOutputs declares, the bound the crate's own helper says applies to exactly those outputs

_Filed: 2026-10-29T11:01:30Z_
_Operation: AddFlowSources | Class: validation_

> _Note: this issue was identified against the code base at commit `1ff82dd4da31f037aed5d48a06cbab4eda93dc7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An ingress (ground-to-cloud) bridge's outputs are the `BridgeFlowOutput`s this service materialises when a cloud flow takes the bridge as its source. The crate reconciles them in one place, `ComAmazonawsMediaconnectMediaconnectState::sync_bridge_flow_outputs`, which every flow-source write calls (CreateFlow, AddFlowSources, UpdateFlowSource, RemoveFlowSource, DeleteFlow). That reconciliation pushes ONE flow output per flow source naming the bridge and applies NO capacity bound at all — so an ingress bridge created with `MaxOutputs: 1` ends up reporting as many outputs as there are flow sources pointing at it, and DescribeBridge answers them.

The crate itself states the opposite rule. `bridge_output_capacity`'s own documentation says the declared ingress maximum is precisely the bound on those materialised outputs, and the function has a dedicated `Some(declared)` arm for the ingress case. That arm is UNREACHABLE: the only caller, `request_bridge_output_rejection`, returns early whenever `ingress_max_outputs.is_some()` (an ingress bridge takes no request-configured output at all), so by the time `bridge_output_capacity(ingress_max_outputs)` is evaluated the argument is always `None`. The ingress bound is therefore declared, documented, stored on the record, reported back by DescribeBridge — and enforced nowhere.

This is not the gap the fixed AddFlowSources issue closed ("a flow taking a bridge as its source never creates the bridge's flow output"): that fix introduced `sync_bridge_flow_outputs`, and left the bound its sibling helper documents unimplemented.

## Evidence

Model — `IngressGatewayBridge$MaxOutputs` is a REQUIRED member: "The maximum number of outputs on the ingress bridge." (smithy_query shape="IngressGatewayBridge").

The crate's own rule, src/state.rs:606-609 (doc of `bridge_output_capacity`):
  "/// What a ground-to-cloud bridge declares bounds the outputs it can ever hold — the flow
   /// outputs the service materialises for it — and not the ones a request configures, of
   /// which it takes none at all (see [`request_bridge_output_rejection`], which refuses
   /// those by the bridge's direction before it counts anything)."
and src/state.rs:610-617 the arm written for that case:
  "pub fn bridge_output_capacity(ingress_max_outputs: Option<i32>) -> usize {
       match ingress_max_outputs {
           Some(declared) => usize::try_from(declared).unwrap_or(0).min(MAX_BRIDGE_OUTPUTS),
           None => MAX_BRIDGE_OUTPUTS,
       }
   }"

That arm is dead — src/state.rs:654-663, the ONLY caller, returns before reaching it for every ingress bridge:
  "    if ingress_max_outputs.is_some() {
           return Some(
               \"An output a request configures is a network output, ... so it accepts no output in a request.\"
               .to_string(),
           );
       }
       let capacity = bridge_output_capacity(ingress_max_outputs);"

The path that DOES put outputs on an ingress bridge applies no bound — src/state.rs:6465-6478 (`sync_bridge_flow_outputs`):
  "        let missing: Vec<&(String, String, String, String)> = requested
               .iter()
               .filter(|(bridge_arn, flow_arn, source_arn, _)| {
                   !held.contains(&(bridge_arn.as_str(), flow_arn.as_str(), source_arn.as_str()))
               })
               .collect();
           for (bridge_arn, flow_arn, source_arn, source_name) in missing {
               let Some(bridge) = self.bridges.get_mut(bridge_arn) else { continue; };
               let name = resolve_bridge_flow_output_name(source_name, &bridge.output_names());
               bridge.outputs.push(materialise_bridge_flow_output(flow_arn, source_arn, name));
           }"
— no reference to `bridge_output_capacity`, `MAX_BRIDGE_OUTPUTS` or `ingress_gateway_bridge.max_outputs` anywhere in the function.

The call site this issue is filed against, src/ops/add_flow_sources.rs:351-356:
  "        // A source that names a bridge is a stream that bridge delivers into the cloud, so
           // the bridge this call pointed a source at owes a flow output for it: ...
           store.sync_bridge_flow_outputs();"

Test coverage states the same rule but never exercises the ingress half — tests/all/bridge_output_admission.rs:11-15:
  "//! takes the bridge as its source, and it takes none a request carries. And whichever
   //! bridge takes them, it holds no more outputs than its own type declares
   //! (`IngressGatewayBridge$MaxOutputs` for a ground-to-cloud bridge; a cloud-to-ground
   //! bridge's `EgressGatewayBridge` declares no output count, so the only bound on it is the
   //! one this crate applies to every bridge, `mc::MAX_BRIDGE_OUTPUTS`)."
The suite drives only CreateBridge/AddBridgeOutputs (request-configured outputs), which an ingress bridge takes none of, so nothing pins the ingress bound. `grep -rn "bridge_output_capacity|MAX_BRIDGE_OUTPUTS" src/` matches only src/state.rs:589-663.

## Steps to Reproduce

1. CreateGateway with one network, and wait for it to settle ACTIVE.
2. CreateBridge with `ingressGatewayBridge: { maxBitrate: 20000000, maxOutputs: 1 }`, a network source, and `placementArn` naming that gateway. DescribeBridge shows the bridge holding 0 outputs and declaring maxOutputs 1.
3. CreateFlow a flow with one source whose `gatewayBridgeSource.bridgeArn` names that bridge. DescribeBridge now reports 1 flow output — the declared maximum is reached.
4. CreateFlow a SECOND flow (or AddFlowSources on another flow) with another source whose `gatewayBridgeSource.bridgeArn` names the same bridge. The call is accepted (200/202).
5. DescribeBridge again: the bridge now reports 2 `flowOutput` entries while its `ingressGatewayBridge.maxOutputs` is still 1 — more outputs than the bridge's own required declaration allows, and more than `bridge_output_capacity(Some(1))` would permit were it ever consulted.
6. Repeat with further flows: the output list grows without bound, limited by nothing.
