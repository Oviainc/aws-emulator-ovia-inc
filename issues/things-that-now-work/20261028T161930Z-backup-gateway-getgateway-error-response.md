Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: ef3dfae520d72d664802e5111b62d3d016960f9c

# The missing-gateway refusal bypasses the crate's single MissingResource choice, so one condition is answered with two different messages

_Filed: 2026-10-28T16:19:30Z_
_Operation: GetGateway | Class: error-response_

> _Note: this issue was identified against the code base at commit `ef3dfae520d72d664802e5111b62d3d016960f9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states that the refusal for a well-formed ARN naming no record it holds is chosen in exactly one place — `crate::state::MissingResource` — and that the sentence is one wording for every operation that resolves an ARN. The hypervisor, virtual-machine and tagging handlers do go through it. The nine gateway handlers do not: each one hand-builds `ResourceNotFoundException { error_code: None, message: Some(format!("The gateway '{}' was not found.", ...)) }` inline, never touching `MissingResource` or `resource_not_found_message`.

The observable consequence is that one and the same condition — a well-formed `gateway/<id>` ARN that this store holds no gateway under — is answered with two different `Message` values by the same crate depending on which operation was asked: `GetGateway` (and `DeleteGateway`, `UpdateGatewayInformation`, `UpdateGatewaySoftwareNow`, `PutMaintenanceStartTime`, `PutBandwidthRateLimitSchedule`, `GetBandwidthRateLimitSchedule`, `TestHypervisorConfiguration`, `DisassociateGatewayFromServer`) says "The gateway '<arn>' was not found.", while `ListTagsForResource`, `TagResource` and `UntagResource` resolve the identical ARN through `MissingResource::for_arn` -> `Other` and say "The resource '<arn>' was not found.". A third wording for the same condition, `GATEWAY_NOT_FOUND_MESSAGE` ("The gateway is not found."), is kept in the state module and used only by `AssociateGatewayToServer`.

Because the status and the `ErrorCode` are chosen inline too, the "cannot be chosen apart" invariant the state module asserts for this refusal holds only for the paths that happen to call the helper.

## Evidence

The rule this crate states for itself — `src/state.rs:2199-2206` (doc of `MissingResource`):

    /// How this service refuses a well-formed resource ARN that names no resource it holds.
    ///
    /// Every such refusal is the modelled `ResourceNotFoundException`, but this service does
    /// not answer alike for all resource types: a missing HYPERVISOR is answered at HTTP 400
    /// with `ErrorCode` [`HYPERVISOR_NOT_FOUND`], where every other resource type takes the
    /// 404 that shape's `@httpError` declares and states no `ErrorCode`. Both halves of a
    /// refusal — the status and the code — are read off this one value, so they cannot be
    /// chosen apart.

and `src/state.rs:2270-2274`:

    /// The sentence `ResourceNotFoundException` states for an ARN this service holds no
    /// record for. One wording for every operation that resolves an ARN.
    pub fn resource_not_found_message(arn: &str) -> String {
        format!("The resource '{arn}' was not found.")
    }

Side that USES the one place — `src/ops/list_tags_for_resource.rs:160-172`:

        let Some((resource_arn, tags)) = state.resource_tags(&input.resource_arn) else {
            let missing = crate::state::MissingResource::for_arn(&input.resource_arn);
            let error = ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    error_code: missing.error_code().map(str::to_string),
                    message: Some(missing.message(&input.resource_arn)),
                },
            );

For a `gateway/<id>` ARN that resolves to `MissingResource::Other` (`src/state.rs:2229-2234`), `message` is `resource_not_found_message(arn)` (`src/state.rs:2252`), i.e. "The resource '<arn>' was not found.". `src/ops/tag_resource.rs:166-178` and `src/ops/untag_resource.rs:171-183` are identical.

Side that SKIPS it — `src/ops/get_gateway.rs:154-165`:

        let Some(gateway) = state.gateway(&input.gateway_arn) else {
            return Err(GetGatewayErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    error_code: None,
                    message: Some(format!(
                        "The gateway '{}' was not found.",
                        input.gateway_arn
                    )),
                },
            )
            .into());
        };

The same inline construction is repeated at `src/ops/delete_gateway.rs:154-165`, `src/ops/update_gateway_information.rs:160-173`, `src/ops/update_gateway_software_now.rs:164-179`, `src/ops/put_maintenance_start_time.rs:231-244`, `src/ops/put_bandwidth_rate_limit_schedule.rs:188-200`, `src/ops/get_bandwidth_rate_limit_schedule.rs:174-187`, `src/ops/test_hypervisor_configuration.rs:195-208` and `src/ops/disassociate_gateway_from_server.rs:182-192`. None of them references `crate::state::MissingResource` (grep for `status_override` lists only the hypervisor/VM/tagging handlers).

The third wording for the same condition, unused by any of the nine — `src/state.rs:186-187`:

    /// See [`GATEWAY_NOT_FOUND`].
    pub const GATEWAY_NOT_FOUND_MESSAGE: &str = "The gateway is not found.";

reached only from `src/ops/associate_gateway_to_server.rs:187-188`:

                            error_code: Some(crate::state::GATEWAY_NOT_FOUND.to_string()),
                            message: Some(crate::state::GATEWAY_NOT_FOUND_MESSAGE.to_string()),

while `src/ops/disassociate_gateway_from_server.rs:178-181` claims the inline sentence is the universal one:

                // A gateway that was never created, or one already deleted, is simply
                // absent from the store — the `ResourceNotFoundException` this operation
                // models, worded the way every other gateway operation of this service
                // words it.

Test coverage: `tests/all/gateway_lifecycle.rs` and `tests/all/negative_paths.rs` assert the shape/status of the refusal; no test pins the gateway-specific sentence against the tagging operations' sentence for the same ARN, so the two wordings are unpinned and free to drift.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. Build a well-formed gateway ARN that no create has minted, e.g. arn:aws:backup-gateway:us-east-1:000000000000:gateway/bgw-0000000000009999 (it satisfies the modelled GatewayArn length 50..180 and pattern, so it reaches the handler).
3. Call GetGateway with that GatewayArn -> HTTP 404 ResourceNotFoundException, Message "The gateway 'arn:aws:backup-gateway:us-east-1:000000000000:gateway/bgw-0000000000009999' was not found.", no ErrorCode.
4. Call ListTagsForResource with ResourceArn = the same string -> HTTP 404 ResourceNotFoundException, Message "The resource 'arn:aws:backup-gateway:us-east-1:000000000000:gateway/bgw-0000000000009999' was not found.".
5. Call AssociateGatewayToServer with that GatewayArn (and any ServerArn) -> HTTP 400 ValidationException, ErrorCode "GatewayNotFound", Message "The gateway is not found." — a third sentence for the same condition, while state.rs:2270-2271 claims "One wording for every operation that resolves an ARN".
