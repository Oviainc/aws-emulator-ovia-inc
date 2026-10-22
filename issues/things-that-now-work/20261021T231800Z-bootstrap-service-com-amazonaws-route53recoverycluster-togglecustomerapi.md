Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the Route53 Recovery Cluster (ToggleCustomerAPI) emulator

_Filed: 2026-10-21T23:18:00Z_
_Operation: service-com-amazonaws-route53recoverycluster-togglecustomerapi | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 4 operations of the `Route53 Recovery Cluster` data-plane service (`awsJson1_0`, `X-Amz-Target: ToggleCustomerAPI.<Op>`, signing name `route53-recovery-cluster`) are unmodified scaffolds answering HTTP 501 and holding zero state. Implement a first stateful slice covering the service's whole core loop — the routing-control on/off toggle — so a realistic client script works end to end.

Happy path (all 4 modeled operations; this service is small enough that the core loop IS the whole API):

1. **ListRoutingControls** — in: `ControlPanelArn` (optional filter), `MaxResults` (int, min 1), `NextToken`; out: `RoutingControls` (required list of `RoutingControl{ControlPanelArn, ControlPanelName, RoutingControlArn, RoutingControlName, RoutingControlState, Owner}`) + `NextToken` when more remain. Must enumerate the routing controls actually held in state, filtered by `ControlPanelArn` when supplied, and honour `MaxResults`/`NextToken` (opaque cursor, 1..8096 chars) rather than returning a fixed list.
2. **GetRoutingControlState** — in: `RoutingControlArn` (required); out: `RoutingControlArn` (required, echo of the looked-up control), `RoutingControlState` (required, `On`|`Off`), `RoutingControlName`. Must read the CURRENT persisted state of that control; unknown ARN -> `ResourceNotFoundException` (404); malformed/empty ARN (len 1..255, pattern `^[A-Za-z0-9:./_-]*$`) -> `ValidationException` (400).
3. **UpdateRoutingControlState** — in: `RoutingControlArn` (required), `RoutingControlState` (required `On`|`Off`), `SafetyRulesToOverride` (list of rule ARNs); out: empty body. Must mutate the stored control so a subsequent GetRoutingControlState returns the new state. Unknown ARN -> `ResourceNotFoundException`; a state value outside `On`/`Off` -> `ValidationException`.
4. **UpdateRoutingControlStates** — in: `UpdateRoutingControlStateEntries` (required list of `UpdateRoutingControlStateEntry{RoutingControlArn (required), RoutingControlState (required)}`), `SafetyRulesToOverride`; out: empty body. Must apply ALL entries atomically against persisted state (validate every entry first, then commit — a failure leaves no partial mutation), so later Get/List calls reflect every entry. More than the service limit of entries (real service caps a batch; >10 entries) -> `ServiceLimitExceededException` (402); duplicate ARNs in one batch or entries spanning more than one cluster -> `ValidationException`; unknown ARN -> `ResourceNotFoundException`.

No canned or echoed responses: every one of the four must compute its response from persisted state. In particular GetRoutingControlState must not echo back a state the caller never set, ListRoutingControls must not return a hardcoded list, and the two update operations must actually persist before returning their empty success body.

Because this is a DATA-plane API, routing controls are created by the separate recovery-control-config control plane and cannot be created through this API. The bootstrap must therefore provide a seeding path so the loop is exercisable: on first touch, materialise routing controls from the shared engine state (e.g. any control panels/routing controls registered by the recovery-control-config service if present), and otherwise auto-register a routing control on first reference by a well-formed ARN with default state `Off`, recording its control panel. Document whichever seeding rule is chosen in `state.rs`.

Safety rules (`SafetyRulesToOverride`) need not be enforced in this first slice — accept and ignore the field, and leave assertion/gating rules to a later gap issue. `ConflictException`, `EndpointTemporarilyUnavailableException`, `ThrottlingException` and `AccessDeniedException` are likewise out of scope for the first slice.

## State Model

In `ComAmazonawsRoute53recoveryclusterTogglecustomerapiState` (reached via `crate::state::service_state(&*self.engine)`, one shared engine per crate):

- `routing_controls: BTreeMap<String /* RoutingControlArn */, RoutingControl>` — insertion/ARN ordered so List pagination is stable, where
  `RoutingControl { routing_control_arn: String, routing_control_name: String, control_panel_arn: String, control_panel_name: String, state: RoutingControlState /* On | Off */, owner: String /* 12-digit account id */ }`
- `control_panels: BTreeMap<String /* ControlPanelArn */, ControlPanel { arn: String, name: String, routing_control_arns: Vec<String> }>` — backs the `ControlPanelArn` filter on ListRoutingControls and the "all entries in one cluster/panel" validation on UpdateRoutingControlStates.
- `list_tokens: HashMap<String /* NextToken */, ListCursor { control_panel_arn: Option<String>, offset: usize }>` — opaque pagination cursors issued by ListRoutingControls.

All mutation happens under the single `state.lock()` guard; UpdateRoutingControlStates validates every entry and then applies them inside ONE lock acquisition so the batch is atomic.

Done = an integration test that lists routing controls, reads one control's state, toggles it On with UpdateRoutingControlState, re-reads it and sees `On`, batch-toggles two controls Off with UpdateRoutingControlStates, re-reads both as `Off`, pages through ListRoutingControls with `MaxResults=1` following `NextToken`, and gets `ResourceNotFoundException` for an unknown ARN — with correct response bodies, not just 2xx.

## Evidence

`services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/OPERATIONS.md` lists exactly 4 operations (GetRoutingControlState, ListRoutingControls, UpdateRoutingControlState, UpdateRoutingControlStates) with the input/output members and errors quoted above, and records the wire protocol as awsJson1_0 with target prefix `ToggleCustomerAPI`. `smithy_query` on `RoutingControl` and `UpdateRoutingControlStateEntry` gives the struct members used in the State Model (ControlPanelArn, ControlPanelName, RoutingControlArn, RoutingControlName, RoutingControlState, Owner / RoutingControlArn+RoutingControlState both @required). All four handlers under `src/ops/` still carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501): update_routing_control_states.rs, list_routing_controls.rs, update_routing_control_state.rs, get_routing_control_state.rs — so the service holds no state at all today. ListRoutingControls carries @paginated(inputToken=NextToken, outputToken=NextToken, items=RoutingControls), grounding the pagination requirement; the On/Off toggle semantics of routing controls (ON = traffic flows to the cell, OFF = it does not) come from the model's own documentation for RoutingControlState.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with service `route53-recovery-cluster` (data plane endpoint).
2. Call ListRoutingControls (optionally with ControlPanelArn) and expect a 200 with the routing controls held in state, each carrying RoutingControlArn, RoutingControlName, ControlPanelArn and RoutingControlState.
3. Call GetRoutingControlState with one returned RoutingControlArn and note the RoutingControlState.
4. Call UpdateRoutingControlState with that ARN and RoutingControlState=On; expect 200 with an empty body.
5. Call GetRoutingControlState again for the same ARN and expect RoutingControlState=On (today: HTTP 501 for every call, nothing is persisted).
6. Call UpdateRoutingControlStates with two entries setting both controls to Off, then GetRoutingControlState on each and expect Off for both.
7. Call ListRoutingControls with MaxResults=1, follow the returned NextToken, and expect the pages to walk the full set without repeats or omissions.
8. Call GetRoutingControlState with an ARN that was never registered and expect ResourceNotFoundException (HTTP 404), not 501.
