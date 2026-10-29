Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8915d54a70fc1484a4f176098767ee5fc740b942

# StartRouteAnalysis reads an endpoint's TransitGatewayAttachmentArn without crate::state::optional_reference, so the empty string is analysed as an unresolvable ARN and stored verbatim

_Filed: 2026-10-29T14:11:40Z_
_Operation: StartRouteAnalysis | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8915d54a70fc1484a4f176098767ee5fc740b942`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RouteAnalysisEndpointOptionsSpecification$TransitGatewayAttachmentArn` is modelled `length {min: 0, max: 500}` with `pattern ^[\s\S]*$`, so the empty string passes the pre-handler constraint walk and reaches the handler. `crate::state::resolve_route_endpoint` reads it with a bare `spec.transit_gateway_attachment_arn.as_deref()` instead of `crate::state::optional_reference`, which state.rs declares as this service's ONE reading of an optional reference member. Three observable consequences:

1. A `Destination` carrying an empty `TransitGatewayAttachmentArn` names no attachment ARN, but `analyze_route_path` tests `to.attachment_arn.is_none()`, which is false for `Some("")`. The path therefore completes with `TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND` instead of `NO_DESTINATION_ARN_PROVIDED`, the reason code state.rs documents for exactly this case.
2. `RouteEndpoint::reason_context` then reports `{"Destination.TransitGatewayAttachmentArn": ""}` — a reason context entry for an ARN that names nothing — although its own doc says the context is `None` "for an endpoint that named no ARN at all".
3. `RouteAnalysisRecord.source` / `.destination` store `RouteAnalysisEndpointOptions { transit_gateway_attachment_arn: Some("") }`, and `GetRouteAnalysis` echoes that empty ARN back for the life of the analysis — a stored reference naming an id nothing can ever be stored under, which `optional_reference` exists to prevent.

Every other handler of this crate that reads an optional reference member whose modelled shape admits the empty string goes through `optional_reference` (`GetNetworkTelemetry`, `GetNetworkResources`, `GetNetworkResourceRelationships`, `GetConnections`, `GetDevices`, `GetLinks`, `GetLinkAssociations`, `AssociateConnectPeer`, `AssociateCustomerGateway`, `AssociateTransitGatewayConnectPeer`, `CreateDevice`, `CreateConnection`). `StartRouteAnalysis` is the one that does not.

## Evidence

Smithy model (`smithy_query query="shape" shape="RouteAnalysisEndpointOptionsSpecification"`): `"TransitGatewayAttachmentArn": {"target": "TransitGatewayAttachmentArn", "length": {"min": 0, "max": 500}, "pattern": "^[\\s\\S]*$"}` — min length 0 and a permissive pattern, so the empty string reaches the handler. (`IpAddress`, by contrast, is `length {min: 1, max: 50}`, so only this member is affected.)

The crate's single rule, services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:470-492:
```
/// constraint walk and reaches the handler. On a member that is OPTIONAL the empty string
/// is the ABSENCE of the reference ... So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is: that keeps every stored record's references either naming a resource of the
/// same global network or naming nothing at all.
...
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```
and the two kinds of member it excludes (state.rs:485-490): "a DESCRIPTIVE one — a link's type or provider, a Region, a state — is not a reference" and "one whose modelled shape does not admit the empty string at all". `TransitGatewayAttachmentArn` is neither: it is looked up as a resource reference and its shape admits the empty string.

The code that skips it, state.rs:5067-5074 (reached from ops/start_route_analysis.rs:202-209 `store.start_route_analysis(...)` -> state.rs:5118-5121 `resolve_route_endpoint`):
```
        let resolution = match spec.transit_gateway_attachment_arn.as_deref() {
            None => RouteEndpointResolution::Unspecified,
            Some(arn) => {
                let held = self
                    .network_resources_of(global_network_id, now)
                    ...
                match held {
                    None => RouteEndpointResolution::Unknown,
```

The reason code the empty string then misses, state.rs:3445-3455:
```
/// * this one — the request named no attachment ARN for the endpoint the path runs
///   TOWARDS, so there is no destination to analyse a path to;
pub const REASON_NO_DESTINATION_ARN_PROVIDED: &str = "NO_DESTINATION_ARN_PROVIDED";
```
state.rs:3666-3667:
```
    let (reason_code, reason_context) = if to.attachment_arn.is_none() {
        (REASON_NO_DESTINATION_ARN_PROVIDED, None)
```
`Some("")` is not `None`, so this branch is not taken.

The reason-context doc the code under it breaks, state.rs:3556-3564:
```
    /// The reason context a path carries when it stopped at this endpoint: the request
    /// member and the ARN the analysis could not resolve, or `None` for an endpoint that
    /// named no ARN at all — there is nothing further to report about it.
    fn reason_context(&self) -> Option<std::collections::HashMap<String, String>> {
        let arn = self.attachment_arn.clone()?;
```

And the verbatim echo back, state.rs:3525-3535 (`RouteEndpoint::options`, stored as `RouteAnalysisRecord.source`/`.destination` at state.rs:5135-5136 and reported by `GetRouteAnalysis` through `to_shape`, state.rs:3634-3635):
```
            transit_gateway_attachment_arn: self.attachment_arn.clone(),
```

Test coverage: the crate already pins this class of defect for other members in tests/all/empty_optional_reference.rs and tests/all/empty_reference_filter.rs, and the route-analysis behaviour is covered by tests/all/route_analysis.rs — which exercises absent and populated ARNs but not the empty-string spelling, so no existing test pins the current answer.

## Steps to Reproduce

1. Create a global network: `CreateGlobalNetwork`.
2. Call `StartRouteAnalysis` on it with `Source={"IpAddress":"10.0.0.1"}` and `Destination={"TransitGatewayAttachmentArn":"","IpAddress":"10.0.1.1"}` (an empty string, which the modelled `length {min:0}` admits).
3. Wait out the settle span and call `GetRouteAnalysis` with the returned `RouteAnalysisId`.
4. Observe `ForwardPath.CompletionStatus.ReasonCode` = `TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND` with `ReasonContext = {"Destination.TransitGatewayAttachmentArn": ""}`, although the request named no attachment ARN for the endpoint the path runs towards — state.rs:3445-3455 documents `NO_DESTINATION_ARN_PROVIDED` for that case, and state.rs:3558 says the context is absent for "an endpoint that named no ARN at all".
5. Observe also `Destination.TransitGatewayAttachmentArn` echoed back as `""` in the stored analysis, a reference naming an id nothing can be stored under — the state state.rs:476-477 says `optional_reference` exists to keep records out of.
6. Compare with the same request where `Destination.TransitGatewayAttachmentArn` is OMITTED: the reason code is `NO_DESTINATION_ARN_PROVIDED`, no reason context, and no ARN reported — the answer the crate's own rule says the empty string must produce too.
