Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# ListGateways reports gateways of every region, ignoring the Region the request was signed for

_Filed: 2026-10-27T12:42:30Z_
_Operation: ListGateways | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `ListGateways` as a listing scoped to one account AND one Region, and this crate already mints every gateway ARN in the region the creating request was signed for (so the store really does hold gateways of several regions once a client addresses more than one endpoint). The handler nevertheless discards the request metadata entirely (`let _ = ctx;`) and pages over the whole gateway map, so a `ListGateways` signed for eu-central-1 reports gateways that only exist in us-west-2, each with an ARN carrying another region's name. The crate already owns the helper this path needs — `crate::state::request_region(ctx.parts())`, which `CreateGateway` and `ImportHypervisorConfiguration` both call to decide the region a record is minted in — and `crate::state::arn_resource_type`/`arn_parts` already split the region segment back out of a stored ARN, so the region of each record is available where the page is cut. No comment, doc or test in the crate states a reason for the listing to span regions; the only neighbouring statement is about the *account* (`state.rs:1154`: "a FILTER on an otherwise whole-account listing"), which says nothing about region.

## Evidence

Smithy (smithy_query(query="doc",shape="ListGateways")), reproduced verbatim in the handler's own module docs at services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/list_gateways.rs:46-47:

    //! Lists backup gateways owned by an Amazon Web Services account in an Amazon Web Services
    //! Region. The returned list is ordered by gateway Amazon Resource Name (ARN).

The handler throws the request away and pages the whole store — src/ops/list_gateways.rs:140-141 and :179:

        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;
...
        let page = state.lock().page_of_gateways(after.as_deref(), limit);

The region is nevertheless part of a gateway's identity in this very crate — src/state.rs:1244-1248 (`create_gateway`):

        let arn = ::engine::emulator_arn(
            ARN_SERVICE,
            &arn_region(region),
            &format!("{GATEWAY_RESOURCE_TYPE}/{gateway_id}"),
        );

and the helper that resolves it per request already exists and is used by the sibling create — src/state.rs:2225-2229:

    pub fn request_region(parts: &::http::request::Parts) -> String {
        ::emulator_server::sigv4_credential_scope(&parts.headers)
            .map(|scope| arn_region(&scope.region))
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

called at src/ops/create_gateway.rs:179 (`let region = crate::state::request_region(ctx.parts());`).

Test coverage: the harness signs every request for one region (tests/all/gateway_lifecycle.rs:42 `pub(crate) const REGION: &str = "us-west-2";`) and only `tests/all/gateway_activation.rs::an_activation_key_is_redeemable_only_in_the_region_it_was_issued_for` ever creates a gateway in a second region (`("a second region, in its own region", "ap-southeast-2", "ap-southeast-2", true)`, tests/all/gateway_activation.rs:632-637) — and that test never calls `ListGateways`, so no existing test pins the cross-region listing.

## Steps to Reproduce

1. Make the activation handshake `GET /?activationRegion=us-west-2` and redeem the issued key with a `CreateGateway` signed for us-west-2; note the returned ARN carries `:us-west-2:`.
2. Make the activation handshake `GET /?activationRegion=ap-southeast-2` and redeem that key with a `CreateGateway` signed for ap-southeast-2; note the returned ARN carries `:ap-southeast-2:`.
3. Call `ListGateways` with an empty body, signed for ap-southeast-2.
4. Observe both gateways in `Gateways`, including the one whose ARN names us-west-2.
5. Expected per the operation's documentation: only the gateway of the Region the request was addressed to.
