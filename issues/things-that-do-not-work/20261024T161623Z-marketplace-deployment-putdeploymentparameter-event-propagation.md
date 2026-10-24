Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# The secretString is never materialised as a Secrets Manager secret, so the written secret is observable nowhere

_Filed: 2026-10-24T16:16:23Z_
_Operation: PutDeploymentParameter | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutDeploymentParameter's documented effect is to create a secret in AWS Secrets Manager for the agreement's acceptor: the model's doc for `PutDeploymentParameterRequest$deploymentParameter` reads "The deployment parameter targeted to the acceptor of an agreement for which to create the AWS Secret Manager resource", and `DeploymentParameterInput$secretString` is "The text to encrypt and store in the secret", with the shape's own note explaining that the `name` must match a CloudFormation template parameter so Quick Launch can resolve it as a Secrets Manager dynamic reference.

Expected: after a successful PutDeploymentParameter, the secret exists in the Secrets Manager plane (this repo emulates it: services/service-com-amazonaws-secretsmanager-secretsmanager), so a client can retrieve/describe the stored secret value and a CloudFormation dynamic reference to it can resolve; a repeat Put for the same (catalog, productId, agreementId, name) updates that secret's value rather than leaving a stale one.

Actual: the handler stores `input.deployment_parameter.secret_string` only in this crate's private `DeploymentParameterRecord` (src/state.rs) and reports nothing about it. This service models no read operation for a deployment parameter (its only reads are the three tagging operations, which report `tags` alone), so the secret the caller wrote is observable through NO API at all — the one documented place it should appear, Secrets Manager, is never written, and no secret is created, updated on a repeat write, or removed when the parameter is purged at its `expirationDate`.

## Evidence

Spec: smithy_query(query="doc", shape="PutDeploymentParameterRequest", member="deploymentParameter") -> "...for which to create the AWS Secret Manager resource"; smithy_query(query="shape", shape="DeploymentParameterInput") -> secretString "The text to encrypt and store in the secret" (@sensitive, length 1..15000) plus the CloudFormation-dynamic-reference note. Well-established AWS behaviour: Marketplace deployment parameters are delivered to the buyer as AWS Secrets Manager secrets, which is what makes {{resolve:secretsmanager:…}} in a Quick Launch template work.

Handler: src/ops/put_deployment_parameter.rs — the create arm writes `secret_string: input.deployment_parameter.secret_string` into `crate::state::DeploymentParameterRecord` and the update arm overwrites `record.secret_string`; nothing else in the crate ever reads that field (grep: only state.rs's record definition and these two writes), and no cross-service write to the Secrets Manager emulator exists. `src/state.rs` documents no propagation seam either.

Test coverage status: tests/all/* (deployment_parameters, put_deployment_parameter_target, front_door_forbidden, …) assert only status codes, tag maps and ARNs; no test observes the written secret anywhere, in this service or in Secrets Manager.

## Steps to Reproduce

1. Seed an agreement for (AWSMarketplace, prod-1a2b3c4d, agmt-1), then POST /catalogs/AWSMarketplace/products/prod-1a2b3c4d/deployment-parameters with {"agreementId":"agmt-1","deploymentParameter":{"name":"DbPassword","secretString":"s3cret"}}.
2. Observe HTTP 200 with resourceArn/deploymentParameterId.
3. Call the Secrets Manager emulator (ListSecrets, or GetSecretValue for the secret the deployment parameter should have created): no secret exists, so the value written in step 1 cannot be retrieved by any caller.
4. Repeat the Put with a new secretString and confirm there is still no Secrets Manager state to update; likewise after the parameter's expirationDate passes and the record is purged, nothing is deleted on the Secrets Manager side.
