Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# A batch delete by ARN list resolves an instrumentation-configuration ARN only in the region it was minted in, so a configuration the service holds earns a per-item ResourceNotFoundException and a short DeletedCount

_Filed: 2026-10-29T00:01:09Z_
_Operation: BatchDeleteInstrumentationConfigurations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The ARN-LIST branch of `BatchDeleteInstrumentationConfigurations` resolves each `ResourceArn` through `crate::state::locate_instrumentation_configuration`, which matches the request's ARN against each stored record's minted ARN as a WHOLE STRING (`configuration.arn == arn`). That comparison pins the ARN's REGION field (and its partition) as part of a configuration's identity.

The crate says the region is NOT part of a resource's identity. `tagged_slo_name_in_arn` states the rule outright — "the region in a minted ARN is a rendering of the writer's SigV4 scope rather than part of an SLO's identity" — and `tests/all/instrumentation_configuration_arn.rs` pins that a configuration's own reads are region-insensitive: "a read reports the one the create minted — even a read whose own credential scope names another region".

So one and the same configuration is judged two ways inside this crate:

* `POST /get-instrumentation-configuration` and `POST /delete-instrumentation-configuration` resolve it by the identity alone (scope + `instrumentation_key`), from any signing region — HTTP 200.
* `POST /batch-delete-instrumentation-configurations` with its ARN differing only in the region segment resolves NOTHING: the item is answered `Code: "ResourceNotFoundException"` and `DeletedCount` does not count it, while the configuration stays stored and readable.

The handler's own doc is the second side of the contradiction: it ENUMERATES exactly which ARNs name no configuration — "one never created, one already deleted, one whose lifetime the clock has run out, one repeated later in the same list, or one of the other instrumentation type". An ARN of this service and account that differs only in its region is none of those five cases, yet it gets the same per-item refusal.

This is a DIFFERENT call site from the one the open TagResource issue (`20261028T162039Z-application-signals-tagresource-wrong-behavior`) describes: that issue cites `tagged_resource_arn` and the three tagging handlers only. `locate_instrumentation_configuration` is a separate helper with its own whole-ARN comparison, reached only by this operation, and a fix confined to `tagged_resource_arn` would leave it diverging from the very precedent its own doc cites.

## Evidence

SIDE THAT PINS THE REGION — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:4806-4833:
```
    /// The scope and [`instrumentation_key`] of the configuration stored under `arn` within
    /// `instrumentation_type`, or `None` when no configuration of that type carries that ARN.
    ///
    /// An `InstrumentationConfigurationArn` spells the service, the environment, the signal
    /// type and the location hash but NOT the instrumentation type, so the type comes from
    /// the request ... The ARN is matched
    /// against the one each stored record was minted with, rather than by taking the identity
    /// apart again, because a service or environment name may itself contain `/` (the same
    /// reason [`Self::tagged_resource_arn`] compares whole ARNs). The scan is bounded by
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn locate_instrumentation_configuration(
        &self,
        instrumentation_type: &str,
        arn: &str,
    ) -> Option<(InstrumentationScope, String)> {
        self.instrumentation_configurations
            .iter()
            .filter(|(scope, _)| scope.instrumentation_type == instrumentation_type)
            ...
            .find(|(_, _, configuration)| configuration.arn == arn)
            .map(|(scope, key, _)| (scope.clone(), key.clone()))
    }
```
The stated justification covers only the RESOURCE field ("a service or environment name may itself contain `/`"); it says nothing about the region.

THE CRATE'S OWN RULE, WHICH THIS BREAKS — src/state.rs:618-620:
```
/// The REGION field is not compared, for the reason the store's own doc gives: this process
/// holds one account's and one Region's records, and the region in a minted ARN is a
/// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
```

THE CRATE'S STATEMENT THAT A CONFIGURATION'S IDENTITY IS FOUR PARTS, NONE OF THEM THE REGION — src/state.rs:4431-4438:
```
/// The ARN of the configuration at `location_hash` in `scope`, in the format the modelled
/// `InstrumentationConfigurationArn` doc states: ...
/// The four parts it spells are a configuration's whole identity — the instrumentation type
/// is not among them — which is the identity a create conflicts on
```

THE HANDLER THAT TURNS THE `None` INTO A PER-ITEM 404 — src/ops/batch_delete_instrumentation_configurations.rs:208-231:
```
            BatchDeleteDeletionTarget::ResourceArns(target) => {
                for resource_arn in &target.resource_arns {
                    let located = store.locate_instrumentation_configuration(
                        &target.instrumentation_type,
                        resource_arn,
                    );
                    match located {
                        Some((scope, key)) => { ... }
                        None => errors.push(BatchDeleteError {
                            resource_arn: resource_arn.clone(),
                            code: crate::state::BATCH_DELETE_ERROR_RESOURCE_NOT_FOUND.to_string(),
                            message: crate::state::instrumentation_configuration_missing(
                                resource_arn,
                            ),
                        }),
```

THE SAME HANDLER'S DOC, WHICH ENUMERATES THE CASES AND DOES NOT INCLUDE THIS ONE — src/ops/batch_delete_instrumentation_configurations.rs:150-158:
```
    /// * By ARN LIST: each ARN is resolved against the stored configurations of the
    ///   instrumentation type the target names ... An ARN naming no such configuration — one never created, one already
    ///   deleted, one whose lifetime the clock has run out, one repeated later in the same
    ///   list, or one of the other instrumentation type — fails that ITEM with the modelled
    ///   `ResourceNotFoundException` code
```

THE READS OF THE SAME RECORD THAT ARE REGION-INSENSITIVE — tests/all/instrumentation_configuration_arn.rs:331-341:
```
        // The ARN is part of the STORED record, so a read reports the one the create minted —
        // even a read whose own credential scope names another region.
        for (who, reader) in [
            ("its own caller", case.caller),
            ("a caller in another region", Caller::Signed(ENDPOINT_REGION)),
        ] {
            let (status, fetched) = get(&router, reader, &case.get_body(hash)).await;
            assert_eq!(
                status, 200,
```

TEST COVERAGE: nothing pins the current behaviour — `grep -i region tests/all/batch_instrumentation_configuration_deletion.rs` returns "No matches", so that suite never addresses a configuration by an ARN of another region, and the region rule is pinned only for the SLO kind (tests/all/resource_tags.rs `an_arn_resolves_whatever_region_it_carries`).

## Steps to Reproduce

1. Start the emulator with this crate mounted and sign requests for us-east-1.
2. POST /create-instrumentation-configuration with {"Service":"checkout","Environment":"eks:prod","SignalType":"SNAPSHOT","InstrumentationType":"PROBE","Location":{"CodeLocation":{"Language":"Java","ClassName":"com.example.CartService","FilePath":"src/main/java/com/example/CartService.java","LineNumber":142}},"CaptureConfiguration":{"CodeCapture":{}}} and note the returned ARN arn:aws:application-signals:us-east-1:<account>:instrumentationConfig/checkout/eks:prod/SNAPSHOT/<hash>.
3. POST /batch-delete-instrumentation-configurations with {"DeletionTarget":{"ResourceArns":{"InstrumentationType":"PROBE","ResourceArns":["arn:aws:application-signals:eu-central-1:<account>:instrumentationConfig/checkout/eks:prod/SNAPSHOT/<hash>"]}}} — the same ARN with only the region segment changed.
4. Observe HTTP 200 with DeletedCount 0, SuccessfulDeletions [] and Errors [{"ResourceArn":"...eu-central-1...","Code":"ResourceNotFoundException","Message":"InstrumentationConfiguration with arn '...' does not exist."}].
5. POST /get-instrumentation-configuration for the same identity (same Service/Environment/SignalType/InstrumentationType/LocationIdentifier), signed in eu-central-1 — observe HTTP 200 returning the record with its us-east-1 ARN, which tests/all/instrumentation_configuration_arn.rs:331-336 pins.
6. Expected: the ARN names the one stored configuration whatever region it carries, because src/state.rs:4435-4437 says the identity an ARN spells is the service, environment, signal type and location hash, and src/state.rs:618-620 says the region in a minted ARN is a rendering of the writer's SigV4 scope rather than part of a resource's identity — so the item should be deleted and counted in DeletedCount, not reported as a missing resource.
