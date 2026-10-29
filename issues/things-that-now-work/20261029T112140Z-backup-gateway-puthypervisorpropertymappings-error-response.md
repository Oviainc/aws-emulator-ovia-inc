Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 05bbbfdc708f909c61f7a8cb3ec61be4744981df

# A HypervisorArn naming no hypervisor is refused two different ways, depending on whether a deleted hypervisor left a stale gateway link behind

_Filed: 2026-10-29T11:21:40Z_
_Operation: PutHypervisorPropertyMappings | Class: error-response_

> _Note: this issue was identified against the code base at commit `05bbbfdc708f909c61f7a8cb3ec61be4744981df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutHypervisorPropertyMappings` gates on `gateway_for_hypervisor` before it resolves the hypervisor record, and the handler states that this gate answers one way for every ARN the store holds no hypervisor under. That is not what the code does, because `gateway_for_hypervisor` never consults the hypervisors map at all — it scans the GATEWAY records for a stale `hypervisor_arn` link, and `remove_hypervisor` deliberately leaves such a link in place when the hypervisor is deleted.

So one condition — "this ARN names no hypervisor this store holds" — is answered with two different error shapes and two different `ErrorCode`s, and which one a caller gets depends on invisible leftover state:

* an ARN no hypervisor was ever imported under (or one deleted while no gateway was associated with it) -> `gateway_for_hypervisor` is `None` -> HTTP 400 `ValidationException` with `ErrorCode` `GatewayOutOfDate` / "The specified gateway is out of date." — the answer the crate's own test pins for "an ARN that names no hypervisor at all";
* an ARN of a hypervisor that was associated with a gateway and then deleted -> the stale link makes `gateway_for_hypervisor` return `Some(gateway)`, the gate passes, `set_property_mappings` finds no record and the handler answers HTTP 400 `ResourceNotFoundException` with `ErrorCode` `HypervisorNotFound` / "The hypervisor is not found."

The crate contradicts itself about this in three places at once: the handler's gate comment says the GatewayOutOfDate refusal is "answered the same way for an ARN this store holds no hypervisor under"; `gateway_for_hypervisor`'s doc asserts it returns `None` "for an ARN this store holds no hypervisor under" (which its body cannot decide, since it looks only at gateway records); and `remove_hypervisor`'s doc asserts the opposite — that a gateway keeps naming a deleted server and that this is exactly what lets the put tell `MissingResource::Hypervisor` from `GATEWAY_OUT_OF_DATE`. Whichever of the two answers is intended, a client branching on (`__type`, `ErrorCode`) sees the service answer one condition two ways, and a maintainer who trusts `gateway_for_hypervisor`'s doc and makes it check the hypervisors map would silently flip the second case to `GatewayOutOfDate` without any test noticing (no test row covers a deleted-but-still-associated hypervisor).

## Evidence

The gate and the rule it claims — `src/ops/put_hypervisor_property_mappings.rs:228-244`:

        // The GATEWAY gate comes first. A property mapping is applied to the hypervisor's
        // entities by the backup gateway appliance that serves it, so a put for a
        // hypervisor no gateway is associated with reaches no appliance that could carry
        // it out: the refusal is about the GATEWAY and not about the mappings or the
        // hypervisor, which is why it is answered the same way for an ARN this store holds
        // no hypervisor under — nothing is associated with a hypervisor the service does
        // not hold either.
        ...
        if store.gateway_for_hypervisor(&input.hypervisor_arn).is_none() {
            return Err(PutHypervisorPropertyMappingsErrorModeled::ValidationException(
                ValidationException {
                    error_code: Some(crate::state::GATEWAY_OUT_OF_DATE.to_string()),

The branch that breaks it — `src/ops/put_hypervisor_property_mappings.rs:246-267`:

        // Past the gate a gateway names this hypervisor. The record it names can still be
        // absent — a hypervisor the store once held and no longer does — and a put cannot
        // set mappings on a record that is not there, so that is refused through the same
        // `crate::state::MissingResource` choice ...
        let Some(updated) = store.set_property_mappings(&input.hypervisor_arn, mappings) else {
            let missing = crate::state::MissingResource::Hypervisor;

The helper whose doc claims the gate cannot be reached that way, next to a body that only scans gateways — `src/state.rs:1943-1956`:

    /// The gateway ASSOCIATED with the hypervisor `hypervisor_arn` names, if any.
    ///
    /// A gateway records the hypervisor it serves in [`StoredGateway::hypervisor_arn`], so
    /// the association is read off the gateway records rather than kept a second time on
    /// the hypervisor record. `None` both for a hypervisor no gateway is associated with
    /// and for an ARN this store holds no hypervisor under — nothing can be associated
    /// with a hypervisor the service does not hold — which is the ONE question an
    /// operation only the serving gateway can carry out is gated on
    /// ([`GATEWAY_OUT_OF_DATE`]).
    pub fn gateway_for_hypervisor(&self, hypervisor_arn: &str) -> Option<&StoredGateway> {
        self.gateways
            .values()
            .find(|gateway| gateway.hypervisor_arn.as_deref() == Some(hypervisor_arn))
    }

The sibling doc that states the OPPOSITE of that sentence, and is the one the code implements — `src/state.rs:1861-1874` (`remove_hypervisor`):

    /// The ASSOCIATION a gateway holds to it does NOT go with it: the link is a fact about
    /// the GATEWAY ([`Self::associate_gateway_to_server`]), which stays its own resource,
    /// and `DisassociateGatewayFromServer` is the operation that clears it. So a gateway
    /// still names the server it was associated with, which is what
    /// [`Self::gateway_for_hypervisor`] resolves and why
    /// `PutHypervisorPropertyMappings` can tell a hypervisor the store no longer holds
    /// ([`MissingResource::Hypervisor`]) from one no gateway serves
    /// ([`GATEWAY_OUT_OF_DATE`]).
    pub fn remove_hypervisor(&mut self, arn: &str) -> Option<StoredHypervisor> {
        let removed = self.hypervisors.remove(arn)?;
        self.virtual_machines
            .retain(|_, machine| machine.hypervisor_arn != arn);

That a stale link is reachable only through delete-after-associate — `src/state.rs:1737-1743` refuses an association whose server is not stored (`AssociationRefusal::NoSuchServer`), so only `DeleteHypervisor` can leave one behind.

Test coverage: `tests/all/put_hypervisor_property_mappings.rs:357-360` and the row at `:397-401` pin the GatewayOutOfDate answer as the rule for ARNs naming no hypervisor —

    /// With no gateway associated with the hypervisor there is no appliance that can apply
    /// the mappings, so the put is refused with HTTP 400 `ValidationException` / `ErrorCode`
    /// `GatewayOutOfDate` ...
        (
            "an ARN that names no hypervisor at all",
            hypervisor_arn_in("aws", "hv-ffffffffffffff11"),
            false,
        ),

and no row of that table (nor any case in `tests/all/hypervisor_property_mappings.rs`) imports a hypervisor, associates a gateway with it, deletes the hypervisor and then puts — so the divergent second answer is untested. `tests/all/missing_resource_refusal_is_one_answer.rs:3-6` states the crate's general rule: "a well-formed ARN of this service that names no record the store holds is refused with the SAME answer whichever operation resolved it — the same status, the same `ErrorCode` member and the same sentence — because the refusal is chosen for the condition and not per operation."

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. GET /?activationRegion=us-east-1&no_redirect to obtain an activation key, then call CreateGateway with it -> note GatewayArn G.
3. Call ImportHypervisorConfiguration {"Name":"hv-stale","Host":"198.51.100.10"} -> note HypervisorArn H.
4. Call AssociateGatewayToServer {"GatewayArn":G,"ServerArn":H} -> 200 (G now records H).
5. Call DeleteHypervisor {"HypervisorArn":H} -> 200. The hypervisor record is gone, but G still carries hypervisor_arn = H.
6. Call PutHypervisorPropertyMappings {"HypervisorArn":H,"IamRoleArn":"arn:aws:iam::000000000000:role/r","VmwareToAwsTagMappings":[{"VmwareCategory":"department","VmwareTagName":"finance","AwsTagKey":"Department","AwsTagValue":"Finance"}]} -> HTTP 400 ResourceNotFoundException with ErrorCode HypervisorNotFound, "The hypervisor is not found."
7. Now call the same PutHypervisorPropertyMappings with a HypervisorArn of the same shape that was never imported (or one imported and deleted without ever being associated) -> HTTP 400 ValidationException with ErrorCode GatewayOutOfDate, "The specified gateway is out of date."
8. Compare: the store holds no hypervisor in either case, yet the two requests get different error shapes and different ErrorCode values, decided by whether a deleted hypervisor left a stale gateway link.
