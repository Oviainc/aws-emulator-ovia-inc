Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0

# The deployment-parameter identity rule is spelled out twice in PutDeploymentParameter (clientToken replay check vs ParameterKey)

_Filed: 2026-10-26T07:27:15Z_
_Operation: PutDeploymentParameter | Class: maintainability_

> _Note: this issue was identified against the code base at commit `cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The identity a deployment parameter is keyed on — (catalog, productId, agreementId, deploymentParameter.name) — is implemented twice inside the same handler: once as a field-by-field comparison in the `clientToken` replay branch, and once as the `ParameterKey` tuple used for the `by_name` index lookup a few lines below. The two copies can drift: adding or renaming a component of `ParameterKey` (whose doc defines the identity) leaves the replay comparison silently judging a different identity, so a token replay could be accepted or turned into a `ConflictException` on the wrong basis. The crate already has the one-place pattern for this on its sibling record type (`ProvisionedAgreement::key`), but `DeploymentParameterRecord` has no equivalent, which is why the handler open-codes the comparison. Deriving the replay comparison from the same key value (e.g. compare the record's key against `key`) leaves every response and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/put_deployment_parameter.rs:319-323`:
```
if record.catalog == input.catalog
    && record.product_id == input.product_id
    && record.agreement_id == input.agreement_id
    && record.name == input.deployment_parameter.name
```
Copy 2 — `src/ops/put_deployment_parameter.rs:337-342`:
```
let key: crate::state::ParameterKey = (
    input.catalog.clone(),
    input.product_id.clone(),
    input.agreement_id.clone(),
    input.deployment_parameter.name.clone(),
);
```
The single definition of that rule, `src/state.rs:475-482`: "/// The identity a deployment parameter is keyed on: the catalog, the product, the agreement and the parameter name. ... pub type ParameterKey = (String, String, String, String);" — and the existing one-place pattern for a sibling record, `src/state.rs:460-464`: "/// The key this agreement is stored under ... pub fn key(&self) -> AgreementKey { (self.catalog.clone(), self.agreement_id.clone()) }".

Existing tests that cover both copies and must stay unchanged: `tests/all/deployment_parameters.rs:979` `a_client_token_identifies_exactly_one_write` (replay returns the identical output, reuse for another parameter conflicts), `tests/all/deployment_parameters.rs:1097` `the_client_token_ledger_is_bounded_and_evicts_the_oldest_token`, and `tests/all/put_deployment_parameter_target.rs:537-576` (the replay row asserting "a clientToken replay must answer the identical output").

## Steps to Reproduce

1. Read src/ops/put_deployment_parameter.rs:316-342 and note the identity (catalog, productId, agreementId, name) is written out twice: once as four `&&`-joined field comparisons for the clientToken replay, once as the `crate::state::ParameterKey` tuple for the `by_name` lookup.
2. Read src/state.rs:475-482 (the ParameterKey definition of that identity) and src/state.rs:460-464 (`ProvisionedAgreement::key`, the same rule expressed once for the sibling type).
3. Observe that `DeploymentParameterRecord` (src/state.rs:343-371) exposes no key() helper, so the handler's replay branch is a second, independent copy of the identity rule.
4. Run the existing suites tests/all/deployment_parameters.rs and tests/all/put_deployment_parameter_target.rs — they pin the replay/conflict behaviour that must be identical after de-duplicating the rule.
