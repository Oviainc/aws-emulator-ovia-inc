Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: outposts.create-order-then-cancel-transitions-to-cancelled outposts.decommission-rejected-for-non-active-lifecycle-status outposts.start-capacity-task-invalid-instance-type-rejected outposts.site-address-not-configured-rejected
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts.create-order-then-cancel-transitions-to-cancelled|run#14|AssertionError|raise AssertionError(
Issue-Key: outposts.decommission-rejected-for-non-active-lifecycle-status|_error_code#9|AssertionError|raise AssertionError("expected start_outpost_decommission to be rejected, but it succeeded")
Issue-Key: outposts.start-capacity-task-invalid-instance-type-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: outposts.site-address-not-configured-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 8dc4d2e19b7449813b28120a27a1ef82acd340ce

# outposts accepts four requests AWS refuses: CreateOrder on an incomplete site, StartOutpostDecommission and StartCapacityTask on a non-ACTIVE Outpost, and GetSiteAddress for an unconfigured AddressType

> _Note: this issue was identified against the code base at commit `8dc4d2e19b7449813b28120a27a1ef82acd340ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCapacityTask

## Description
4 net-new root causes, all of one kind: a precondition AWS enforces is absent in the emulator, so a call AWS refuses with a 4xx is answered 200 (and in one case with a success body that is missing the very member the caller asked for). Every cause below is grounded in a behavioral check that reached the operation and got a 200.

Related OPEN issue that does NOT cover these as filed: 20261029T191039Z-outposts-lifecycle-and-precondition-gates-diverge-across-capacity describes the StartCapacityTask/StartOutpostDecommission lifecycle gates from the WIRE corpus and carries no behavioral replay key for the checks listed here; 20261029T074713Z-outposts-createorder-validation--r2 (DISCONTINUED catalog configuration) and 20261029T004219Z-outposts-createorder-wrong-behavior (AssetInformationList) are different CreateOrder causes; no open issue mentions GetSiteAddress at all.

Cause 1 of 4: CreateOrder against an Outpost whose Site has no completed site details.
AWS: 400 ValidationException (Smithy: CreateOrder errors include ValidationException errorKind client httpError 400; also NotFoundException 404, ConflictException 409, ServiceQuotaExceededException 402) with the wholly FIXED Message `The site details are incomplete. Reach out to AWS Support for assistance.` — no variable parts; this is recorded behavior only (the model documents no message text for it). AWS requires the Site's address/rack qualification to be complete before an order can be placed, so a Site created with only Name/Tags can never carry an order, and ListOrders for the Outpost stays empty.
Emulator: 200 with a full Order body (an OrderId is minted and the order is listable), for an Outpost whose Site was created with no OperatingAddress, no ShippingAddress and no RackPhysicalProperties.
Why: a client that stages CreateSite -> CreateOutpost -> CreateOrder never learns that site qualification is a prerequisite; it proceeds with an OrderId AWS would never have issued, and the downstream fulfilment/capacity flow is reachable from a state AWS forbids. Any test asserting the refusal silently passes a call that must fail.
Fix: CreateOrder's request validation in the outposts crate's CreateOrder handler (services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_order.rs) — resolve the Outpost's Site and refuse with the modeled ValidationException when the site's details are not complete, before any Order is persisted.
Traces: outposts.create-order-then-cancel-transitions-to-cancelled.

Cause 2 of 4: StartOutpostDecommission on an Outpost that has never reached LifeCycleStatus ACTIVE — including with ValidateOnly=true.
AWS: 400 ValidationException (Smithy: StartOutpostDecommission errors include ValidationException httpError 400; input member ValidateOnly has default false) whose Message names the Outpost and the status that blocks it: fixed text `Outpost <outpostId> has invalid lifecycleStatus=<status>` — the variable parts are the Outpost id resolved from the request's OutpostIdentifier and the Outpost's CURRENT LifeCycleStatus from service state (PENDING for a freshly registered Outpost). ValidateOnly=true does NOT convert the refusal into a SKIPPED/BLOCKED plan: the precondition is checked first. The Outpost is unaffected — GetOutpost afterwards still returns it with the same LifeCycleStatus.
Emulator: 200 with a decommission response; no error at all, so the decommission state machine is entered from a state AWS forbids.
Why: a caller can decommission hardware that was never installed; GetOutpost then reports a lifecycle AWS cannot produce, and tooling that relies on the refusal to detect "not installed yet" gets a false green.
Fix: StartOutpostDecommission's precondition check (src/ops/start_outpost_decommission.rs) — gate on the Outpost's LifeCycleStatus before producing any plan, on both the ValidateOnly and the real path.
Traces: outposts.decommission-rejected-for-non-active-lifecycle-status.

Cause 3 of 4: StartCapacityTask on an Outpost that is not ACTIVE.
AWS: 400 ValidationException (Smithy: StartCapacityTask errors include ValidationException httpError 400) with Message `Failed to create capacity task for <outpostId>. Outpost should have LifeCycleStatus being ACTIVE, but <outpostId> has LifeCycleStatus <status>` — fixed text exactly as quoted, with the resolved Outpost id (appearing twice) and the Outpost's current LifeCycleStatus from state as the only variable parts. No capacity task is created, so ListCapacityTasks filtered to the Outpost stays empty.
Emulator: 200 with a capacity-task body — CapacityTaskId minted, CapacityTaskStatus REQUESTED, CreationDate/LastModifiedDate set — for a freshly created, hardware-less Outpost (LifeCycleStatus PENDING). The same check confirms the InstanceType format validation on the SAME operation is correct (a malformed InstanceType is properly refused with the modeled constraint message), so this is specifically the missing lifecycle gate, not a broken handler.
Why: a client that stages "create Outpost, then configure capacity" never learns it must wait for ACTIVE; it polls a task id AWS never issued, and the negative-control distinction between "malformed instance type" and "Outpost not ACTIVE" collapses.
Fix: StartCapacityTask's precondition checks (src/ops/start_capacity_task.rs) — gate on the Outpost's LifeCycleStatus after format validation and before the task is minted.
Traces: outposts.start-capacity-task-invalid-instance-type-rejected.

Cause 4 of 4: GetSiteAddress for an AddressType that was never configured on an existing Site.
AWS: 404 NotFoundException (Smithy: GetSiteAddress errors include NotFoundException errorKind client httpError 404; input AddressType is a required httpQuery enum; output member Address is optional and sensitive) with Message `<AddressType> not found for SiteId=<siteId>` — fixed text " not found for SiteId=" with the requested AddressType enum value (e.g. SHIPPING_ADDRESS) and the resolved site id as the variable parts. The refusal tracks CONFIGURATION state, not input validity: the AddressType that WAS configured on the same Site reads back 200 with its Address, and configuring the missing one via UpdateSiteAddress makes the identical GetSiteAddress call succeed.
Emulator: 200 with a body carrying only SiteId and AddressType and NO Address member (recorded response: content-length 66, `{SiteId, AddressType: SHIPPING_ADDRESS}`), because Address is optional in the model and the handler simply omits it instead of refusing. The positive control (OPERATING_ADDRESS, which was set) does read back correctly.
Why: a caller gets a 200 it must then defensively inspect for a missing Address, and cannot distinguish "this site has no shipping address configured" from "the address is withheld"; code that branches on NotFoundException to decide whether to call UpdateSiteAddress never takes that branch. Because the check stopped here, the UpdateSiteAddress-then-GetSiteAddress restore leg on the same Site is unverified and should be re-replayed with the fix.
Fix: GetSiteAddress's lookup in the GetSiteAddress handler (src/ops/get_site_address.rs) — after resolving the Site, raise the modeled NotFoundException when the requested AddressType has no address recorded, instead of serializing a success with Address absent.
Traces: outposts.site-address-not-configured-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (outposts 18/25 passed):

- [mismatch] outposts.create-order-then-cancel-transitions-to-cancelled: "CreateOrder unexpectedly SUCCEEDED (OrderId='oo-a6e96023c560fbe12', OutpostId='op-54fc024b0261361ac')". The check created a Site with only Name/Tags and an Outpost on it, then called CreateOrder with one AVAILABLE catalog line item and PaymentOption=NO_UPFRONT; it expects ClientError code ValidationException with Message exactly "The site details are incomplete. Reach out to AWS Support for assistance." and ListOrders(OutpostIdentifierFilter=<outpost>) == []. The ids quoted are per-run fillers, not requirements.

- [mismatch] outposts.decommission-rejected-for-non-active-lifecycle-status: "expected start_outpost_decommission to be rejected, but it succeeded". GetOutpost first confirmed LifeCycleStatus != ACTIVE; start_outpost_decommission(OutpostIdentifier=<id>, ValidateOnly=True) then returned success instead of ValidationException whose Message contains both <outpostId> and that status.

- [mismatch] outposts.start-capacity-task-invalid-instance-type-rejected: the malformed-InstanceType leg PASSED (code ValidationException, Message "1 validation error detected: Value at 'instancePools.1.member.instanceType' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9\-]+\.[a-z0-9\-]+$"). The positive-control leg FAILED: start_capacity_task(OutpostIdentifier=<id>, InstancePools=[{InstanceType: c5.xlarge, Count: 1}]) returned HTTPStatusCode 200 with {'CapacityTaskId': 'cap-76e27b931be897d29', 'OutpostId': 'op-96c40dd86d198cfa1', 'RequestedInstancePools': [{'InstanceType': 'c5.xlarge', 'Count': 1}], 'DryRun': False, 'CapacityTaskStatus': 'REQUESTED', 'CreationDate': ..., 'LastModifiedDate': ...}, where AWS answers 400 ValidationException "Failed to create capacity task for <outpostId>. Outpost should have LifeCycleStatus being ACTIVE, but <outpostId> has LifeCycleStatus PENDING".

- [mismatch] outposts.site-address-not-configured-rejected: get_site_address(SiteId=<id>, AddressType='SHIPPING_ADDRESS') on a Site created with OperatingAddress only returned HTTPStatusCode 200, body {'SiteId': 'os-260ea088248d1d21c', 'AddressType': 'SHIPPING_ADDRESS'} with no 'Address' member, where AWS answers 404 NotFoundException with Message "SHIPPING_ADDRESS not found for SiteId=<siteId>". The same Site's OPERATING_ADDRESS read back 200 with Address.City == 'Seattle' (positive control passed), so the Site itself resolves.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite (service outposts).
2. Cause 1 — CreateSite(Name only) -> CreateOutpost(SiteId, AvailabilityZone) -> CreateOrder(OutpostIdentifier=<outpost>, LineItems=[{CatalogItemId from an AVAILABLE ListCatalogItems entry, Quantity 1}], PaymentOption=NO_UPFRONT). Emulator answers 200 with an Order; AWS answers 400 ValidationException "The site details are incomplete. Reach out to AWS Support for assistance." and ListOrders for the Outpost stays empty.
3. Cause 2 — on the same kind of freshly registered Outpost, GetOutpost to read its non-ACTIVE LifeCycleStatus, then POST /outposts/{id}/decommission with ValidateOnly=true. Emulator answers 200; AWS answers 400 ValidationException naming the Outpost id and that status, and GetOutpost afterwards still reports the unchanged status.
4. Cause 3 — on a freshly registered (PENDING) Outpost, POST /outposts/{id}/capacity with InstancePools=[{InstanceType:"c5.xlarge",Count:1}]. Emulator answers 200 with CapacityTaskStatus REQUESTED; AWS answers 400 ValidationException with the LifeCycleStatus sentence, and ListCapacityTasks(OutpostIdentifierFilter=<outpost>) stays empty.
5. Cause 4 — CreateSite with OperatingAddress and deliberately no ShippingAddress, then GET /sites/{siteId}/address?AddressType=SHIPPING_ADDRESS. Emulator answers 200 with Address omitted; AWS answers 404 NotFoundException "SHIPPING_ADDRESS not found for SiteId=<siteId>". Then UpdateSiteAddress(AddressType=SHIPPING_ADDRESS, Address=...) and repeat the same GET — it must now answer 200 with that Address (restore leg, currently unverified because the check stops at the refusal).
