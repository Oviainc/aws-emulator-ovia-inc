Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# A fresh emulator holds no agreements, so every PutDeploymentParameter is a 404

_Filed: 2026-10-24T01:02:35Z_
_Operation: PutDeploymentParameter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutDeploymentParameter is the only write operation this service models, and the handler refuses the write unless the (catalog, productId, agreementId) triple is already present in `state.agreements` (populated only by the in-process Rust seam `crate::state::seed_agreements`). The agreement store is `#[derive(Default)]`-empty when the emulator starts and NOTHING reachable over HTTP can add to it — this model declares no CreateAgreement/CreateProduct operation and the crate exposes no admin route or fixture seeding at construction.

Expected (real service, a caller whose account holds the agreement the request names): HTTP 200 with a minted `resourceArn`/`deploymentParameterId`, and the record afterwards addressable by the tagging operations.

Actual (emulator driven over the wire, e.g. by an SDK client or a replayed recording that carries a real productId/agreementId): HTTP 404 `ResourceNotFoundException` ("ProductId: Request references a resource which does not exist." / "AgreementId: …") for every single request, and nothing is stored. The create -> read (ListTagsForResource) -> update -> untag lifecycle can therefore never be exercised by an HTTP client; it only works from in-process Rust tests that call `seed_agreements` first.

The 404 for an unheld target is itself correct service behaviour — the gap is that the emulator has no state in which any target is held, and no wire-reachable way to get there (e.g. pre-seeded fixture agreements in the state's `Default`, auto-provisioning the target named by a well-formed request, or an admin seam a client can reach).

## Evidence

Model: `PutDeploymentParameter` (POST /catalogs/{catalog}/products/{productId}/deployment-parameters -> 200) is the service's only non-tagging operation; the service binds only ListTagsForResource/TagResource/UntagResource plus this one, so no modelled operation can create a product or agreement. Handler: services/.../src/ops/put_deployment_parameter.rs lines 253-260 return `ResourceNotFoundException` whenever `store.missing_target(catalog, product_id, agreement_id)` is `Some`. State: services/.../src/state.rs — `ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState` is `#[derive(Debug, Default)]` (empty `agreements`), and `seed_agreements` (state.rs ~line 635) is a Rust function taking `&Eng`, not an HTTP surface. Tests: tests/all/deployment_parameters.rs `provision()` calls `seed_agreements` before every successful put, so existing coverage only proves the seeded path; no test (and no client) can reach a 200 without the in-process seam.

## Steps to Reproduce

1. Start the emulator fresh (no in-process seeding).
2. POST /catalogs/AWSMarketplace/products/prod-123/deployment-parameters with body {"agreementId":"agmt-123","deploymentParameter":{"name":"DbPassword","secretString":"s3cret"}} signed for aws-marketplace.
3. Observe HTTP 404 ResourceNotFoundException instead of a 200 carrying resourceArn/deploymentParameterId.
4. Repeat with any productId/agreementId pair: every request is a 404, so GET /tags/{resourceArn} can never find a record created through the API.
