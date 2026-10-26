Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.update-endpoint-nonexistent-not-found events.describe-endpoint-nonexistent-not-found events.create-endpoint-malformed-routing-config-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.update-endpoint-nonexistent-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"UpdateEndpoint on {bogus}: got {code}"
Issue-Key: events.describe-endpoint-nonexistent-not-found|run#7|AssertionError|assert err == ("ResourceNotFoundException", f"Endpoint {bogus} does not exist in region {ctx.region}."), (
Issue-Key: events.create-endpoint-malformed-routing-config-rejected|run#9|AssertionError|assert describe_err == expected_absent, (
Issue-Commit: e2bc5ee527b441152171cb1bdafdba5e85732812

# EventBridge global endpoints: UpdateEndpoint is an unimplemented 501 stub, and the endpoint ResourceNotFoundException message omits the region clause (2 causes)

> _Note: this issue was identified against the code base at commit `e2bc5ee527b441152171cb1bdafdba5e85732812`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEndpoint

## Description
Two independent, net-new defects in the EventBridge Global Endpoint surface. Both were reached AFTER the emulator answered CreateEndpoint and DescribeEndpoint successfully (the positive controls in both checks passed), so neither is a bootstrap gap.

Neither cause is covered by the open issue 20261025T002357Z-events-eventbridge-state-and-representation-diverge-describeconnection-leaks-the.md. That issue's Cause 2 (the event-bus namespace is not partitioned by region) blocks events.update-endpoint-changes-routing-config and events.endpoint-lifecycle-create-active-delete-404 at a CreateEventBus call in a second region, before any endpoint assertion runs. The two checks below create their endpoints from the account's always-present `default` buses, so they get past that refusal and expose these two further defects. The fixed issue 20261025T002357Z-events-describeendpoint-stub.md did implement DescribeEndpoint — the operation now serves and the positive control describes fine, so this is not a reopen of it; only the text of its not-found message is wrong.

Cause 1 of 2 — UpdateEndpoint is an unimplemented scaffold stub: every call answers 501 NotImplemented.
AWS: Smithy `UpdateEndpoint` (com.amazonaws.eventbridge#UpdateEndpoint) is a real operation: success status 200 returning Name, Arn, RoutingConfig, ReplicationConfig, EventBuses, RoleArn, EndpointId, EndpointUrl and State; modeled errors are ConcurrentModificationException (HTTP 400, "There is concurrent modification on a rule, target, archive, or replay."), InternalException (HTTP 500) and ResourceNotFoundException (HTTP 400, "An entity that you specified does not exist."). For a `Name` that was never created, AWS refuses with ResourceNotFoundException/400 and the message `Endpoint <Name> does not exist in region <region>.` — fixed text "Endpoint " ... " does not exist in region " ... ".", with `<Name>` taken from the request's Name member and `<region>` the region the request was addressed to. For an endpoint that does exist, UpdateEndpoint(Name=..., Description=...) answers 200 and the new Description becomes visible through DescribeEndpoint.
Emulator: every UpdateEndpoint request — existent or not — is answered HTTP 501 with error code `NotImplemented`. The replay saw code `NotImplemented` where ResourceNotFoundException was required. The handler still returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateEndpoint"))` and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line.
Why: `NotImplemented`/501 is not one of UpdateEndpoint's modeled errors, so an SDK client sees an unmodeled, unretryable fault instead of the ResourceNotFoundException it is written to catch, and it cannot tell a missing endpoint from a server failure. More broadly no global endpoint can ever be mutated: Description, RoutingConfig, ReplicationConfig, RoleArn and EventBuses are all immutable in the emulator, so failover reconfiguration and replication enable/disable are unreachable.
Fix: the operation's handler — implement `UpdateEndpoint::handle` in services/service-com-amazonaws-eventbridge-awsevents/src/ops/update_endpoint.rs against the existing endpoint record in services/service-com-amazonaws-eventbridge-awsevents/src/state.rs (EndpointRecord / settle_endpoints / endpoint_in_home_region), and delete the SCAFFOLDED marker line.
Traces: events.update-endpoint-nonexistent-not-found.

Cause 2 of 2 — the endpoint not-found message omits the " in region <region>" clause.
AWS: Smithy `DescribeEndpoint` models ResourceNotFoundException (HTTP 400) — "An entity that you specified does not exist." — and takes an optional `HomeRegion` member alongside the required `Name`, because a global endpoint is homed in one region. Recorded behavior: DescribeEndpoint on a name that does not exist answers ResourceNotFoundException/400 with message `Endpoint <Name> does not exist in region <region>.` The fixed text is "Endpoint ", " does not exist in region " and the trailing "."; the variable parts are the request's `Name` member and the resolved region (the request's `HomeRegion` when supplied, otherwise the region the request was addressed to).
Emulator: answers the right code and status (ResourceNotFoundException/400) but the message `Endpoint <Name> does not exist.` — the whole " in region <region>" clause is dropped. The message is built by `crate::state::no_such_endpoint(&input.name)`, which is given only the name and has no region to render.
Why: the message is the only place the response says WHICH region was searched, and global endpoints are explicitly region-homed. A client (or a CloudFormation/IaC reconciler) that reads the message cannot distinguish "this endpoint exists nowhere" from "it is not homed in the region you asked", and any client matching on AWS's message text fails to match. A diverging error message is itself a contract break.
Fix: the not-found message builder for endpoints — `no_such_endpoint` in services/service-com-amazonaws-eventbridge-awsevents/src/state.rs — must take and render the resolved region, and its callers must pass it: services/service-com-amazonaws-eventbridge-awsevents/src/ops/describe_endpoint.rs:185, plus the same message on DeleteEndpoint/UpdateEndpoint once those are implemented.
Traces: events.describe-endpoint-nonexistent-not-found, events.create-endpoint-malformed-routing-config-rejected (in the latter, CreateEndpoint's ValidationException for the missing `routingConfig.failoverConfig.secondary` matched AWS exactly; the check then failed only on the follow-up DescribeEndpoint effect-absent message).

## Evidence
Cause 1 — UpdateEndpoint:
  Check events.update-endpoint-nonexistent-not-found, assertion at check_events_update_endpoint_nonexistent_not_found.py:28.
  Expected: code `ResourceNotFoundException` (HTTP 400), message `Endpoint <bogus-name> does not exist in region <region>.`
  Got: `AssertionError: UpdateEndpoint on sim2real-behav-noendpoint-f279cc2c2b39: got NotImplemented` — i.e. HTTP 501 / error code `NotImplemented`, which is not a modeled UpdateEndpoint error at all.
  Source confirmation: services/service-com-amazonaws-eventbridge-awsevents/src/ops/update_endpoint.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 186 is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateEndpoint"))`.
  The same check's positive control (UpdateEndpoint on the endpoint that does exist, then DescribeEndpoint showing the new Description) is therefore also unreachable.

Cause 2 — endpoint not-found message:
  Check events.describe-endpoint-nonexistent-not-found, assertion at check_events_describe_endpoint_nonexistent_not_found.py:31.
  Expected: `('ResourceNotFoundException', 'Endpoint <bogus-name> does not exist in region <region>.')`
  Got:      `('ResourceNotFoundException', 'Endpoint sim2real-behav-noendpoint-071990c8f9ec does not exist.')`
  Status and error code are correct; only the message diverges — the " in region <region>" clause is absent.

  Check events.create-endpoint-malformed-routing-config-rejected, assertion at check_events_create_endpoint_malformed_routing_config_rejected.py:60.
  CreateEndpoint with FailoverConfig missing the required Secondary was correctly refused (ValidationException, "1 validation error detected: Value null at 'routingConfig.failoverConfig.secondary' failed to satisfy constraint: Member must not be null"). The check then asserts the rejected create left no describable endpoint:
  Expected: `('ResourceNotFoundException', 'Endpoint <name> does not exist in region <region>.')`
  Got:      `('ResourceNotFoundException', 'Endpoint sim2real-behav-ep-c2a248a2f400 does not exist.')`
  (Names, ids and the region literal above are per-run values filling the message template; the requirement is the template, not these values.)

Smithy citations used: UpdateEndpoint — input {Name (required, EndpointName, len 1..64, pattern ^[\.\-_A-Za-z0-9]+$), Description, RoutingConfig, ReplicationConfig, EventBuses (len 2..2), RoleArn}; output {Name, Arn, RoutingConfig, ReplicationConfig, EventBuses, RoleArn, EndpointId, EndpointUrl, State}; errors ConcurrentModificationException/400, InternalException/500, ResourceNotFoundException/400; success 200. DescribeEndpoint — input {Name (required), HomeRegion}; errors InternalException/500, ResourceNotFoundException/400.

## Steps to Reproduce
Cause 1 (UpdateEndpoint stub):
1. Call `events.update_endpoint(Name='<a-name-that-was-never-created>', Description='should-not-apply')`.
   -> Emulator: HTTP 501, error code `NotImplemented`. AWS: HTTP 400 `ResourceNotFoundException`, message `Endpoint <name> does not exist in region <region>.`
2. Create a real global endpoint (CreateEndpoint with RoutingConfig.FailoverConfig Primary.HealthCheck = a real Route53 health-check ARN, Secondary.Route = a second region, ReplicationConfig State=DISABLED, EventBuses = the two regions' `default` bus ARNs), wait for State=ACTIVE, then call `events.update_endpoint(Name=<that-name>, Description='updated')`.
   -> Emulator: HTTP 501 `NotImplemented`. AWS: HTTP 200, and DescribeEndpoint(Name=<that-name>) then reports Description='updated'.

Cause 2 (not-found message):
3. Call `events.describe_endpoint(Name='<a-name-that-was-never-created>')`.
   -> Emulator: ResourceNotFoundException with message `Endpoint <name> does not exist.` AWS: ResourceNotFoundException with message `Endpoint <name> does not exist in region <region>.`
4. Call CreateEndpoint (with client-side parameter validation disabled) with RoutingConfig={'FailoverConfig': {'Primary': {'HealthCheck': 'arn:aws:route53:::healthcheck/00000000-0000-0000-0000-000000000000'}}} — i.e. the required `Secondary` omitted — and two EventBuses ARNs; the call is correctly refused ValidationException. Then call `events.describe_endpoint(Name=<that-same-name>)`.
   -> Same message divergence as step 3.
