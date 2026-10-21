Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 36a9702edf97929891a031643d156f194fedffe9

# Bootstrap the AWS Marketplace Deployment (AWSMPDeploymentParametersService) emulator

_Filed: 2026-10-21T23:08:25Z_
_Operation: service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice | Class: stub_

> _Note: this issue was identified against the code base at commit `36a9702edf97929891a031643d156f194fedffe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this service's 4 operations is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state. Implement a first functional slice covering the service's whole surface, which is the deployment-parameter lifecycle plus its tagging loop.

Happy path to implement, in order:

1. **PutDeploymentParameter** — `POST /catalogs/{catalog}/products/{productId}/deployment-parameters`. Input: `catalog` (@httpLabel, len 1..64, `^[a-zA-Z_-]+$`), `productId` (@httpLabel, len 1..64, `^[A-Za-z0-9_/-]+$`), `agreementId` (required, len 1..64), `deploymentParameter` (required, `DeploymentParameterInput{name` required len 1..400 `^[a-zA-Z0-9/_+=.@-]+$`, `secretString` required len 1..15000, @sensitive`}`), optional `tags` (map<string,string>, max 50), `expirationDate` (timestamp), `clientToken` (@idempotencyToken, len 32..64). Must MINT and PERSIST a record: generate `deploymentParameterId` matching the modeled pattern `^dp-[a-zA-Z0-9]+$` (len 1..32) and a `resourceArn` in the `aws-marketplace` ARN namespace (e.g. `arn:aws:aws-marketplace:<region>:<account>:deployment-parameter/<catalog>/<productId>/dp-xxxx`), store the secretString, agreementId, expirationDate and tags, and return `resourceArn`, `agreementId`, `deploymentParameterId`, `tags` computed from the stored record — not echoed blindly. The operation is `@idempotent` and keyed on the parameter NAME for a given (catalog, productId, agreementId): a second call with the SAME `deploymentParameter.name` must UPDATE the existing record and return the SAME `deploymentParameterId`/`resourceArn`, not mint a new one. A repeat with the same `clientToken` must likewise be a no-op replay returning the identical output.
2. **TagResource** — `POST /tags/{resourceArn}` -> **204** with an empty body. Must merge `tags` into the tag map of the record whose stored `resourceArn` matches the @httpLabel, and return `ResourceNotFoundException` (404) for an ARN that was never created.
3. **ListTagsForResource** — `GET /tags/{resourceArn}` -> 200 with `tags`. Must return exactly the merged tag map persisted by PutDeploymentParameter + TagResource for that ARN, and `ResourceNotFoundException` (404) for an unknown ARN. This is the read that proves state is real.
4. **UntagResource** — `DELETE /tags/{resourceArn}?tagKeys=...` -> **204**. Must remove the listed `tagKeys` (required @httpQuery list) from the stored tag map; `@idempotent`, so removing a key that is already absent still succeeds with 204. A follow-up ListTagsForResource must no longer show the removed keys.

Rules for all four: no canned, hardcoded or echoed responses. Every response field must be computed from the persisted record, and every write must be visible to a later read in the same process. Missing/empty required members, labels violating the modeled length/pattern constraints, or a `tags` map over 50 entries must fail with `ValidationException` (400), not a 2xx. Unknown ARNs fail with `ResourceNotFoundException` (404). Reach the store through `self.engine` / `crate::state::service_state` (never a fresh `InMemoryEngine`).

## State Model

In `src/state.rs`, on the service state struct:

- `parameters: HashMap<String, DeploymentParameterRecord>` — keyed by the minted `resource_arn` (the key both tagging ops resolve by).
- `by_name: HashMap<(String /*catalog*/, String /*product_id*/, String /*agreement_id*/, String /*parameter_name*/), String /*resource_arn*/>` — the idempotency index that makes a repeat Put update-in-place.
- `client_tokens: HashMap<String /*client_token*/, String /*resource_arn*/>` — idempotency-token replay index.
- `DeploymentParameterRecord { resource_arn: String, deployment_parameter_id: String /* "dp-" + alphanumeric */, catalog: String, product_id: String, agreement_id: String, name: String, secret_string: String /* sensitive — never logged */, expiration_date: Option<Timestamp>, tags: BTreeMap<String,String> /* max 50 */, created_at: Timestamp, updated_at: Timestamp }`
- a monotonic `next_id: u64` (or equivalent) used only to make `dp-` ids unique.

Done when a client can Put a deployment parameter, tag it, list the tags and see both the Put-time and TagResource-time tags, untag a key and see it gone, and re-Put the same parameter name and get the same identifiers back — all with correct status codes (200/204/404/400).

## Evidence

OPERATIONS.md for this crate lists exactly 4 operations (PutDeploymentParameter, TagResource, UntagResource, ListTagsForResource) with the routes, @httpLabel/@httpQuery bindings, required members, length/pattern constraints and error set cited above; protocol `restJson1`, sigv4 signing name `aws-marketplace`. `smithy_query shape=DeploymentParameterInput` confirms `name` (required, len 1..400, `^[a-zA-Z0-9/_+=.@-]+$`) and `secretString` (required, len 1..15000, @sensitive), and documents that the name 'is the identifier on which deployment parameters are keyed for a given buyer and product. If this name matches an existing deployment parameter' it is updated — grounding the update-in-place idempotency requirement. All four handler files (`src/ops/put_deployment_parameter.rs`, `tag_resource.rs`, `untag_resource.rs`, `list_tags_for_resource.rs`) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exercise any behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `marketplace-deployment` client at it.
2. Call PutDeploymentParameter with catalog=`AWSMarketplace`, productId=`prod-abc123`, agreementId=`agmt-abc123`, deploymentParameter={name:`DbPassword`, secretString:`s3cr3t`}, tags={`env`:`dev`} — expect 200 with a `resourceArn`, the echoed `agreementId`, a `deploymentParameterId` matching `^dp-[a-zA-Z0-9]+$`, and `tags` containing env=dev (currently 501).
3. Call TagResource on the returned resourceArn with tags={`team`:`platform`} — expect HTTP 204 and an empty body.
4. Call ListTagsForResource on that resourceArn — expect 200 with BOTH tags: env=dev and team=platform, proving the Put-time and Tag-time writes persisted to the same record.
5. Call UntagResource on that resourceArn with tagKeys=[`env`] — expect 204; repeat the same call and expect 204 again (idempotent).
6. Call ListTagsForResource again — expect only team=platform.
7. Call PutDeploymentParameter again with the same catalog/productId/agreementId and deploymentParameter.name=`DbPassword` but secretString=`rotated` — expect 200 with the SAME deploymentParameterId and resourceArn as step 2 (update in place, not a new resource).
8. Call ListTagsForResource with a fabricated ARN — expect ResourceNotFoundException (HTTP 404), not 501 and not 200.
9. Call PutDeploymentParameter with catalog=`bad catalog 1!` (violates `^[a-zA-Z_-]+$`) — expect ValidationException (HTTP 400).
