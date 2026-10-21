Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the codeartifact emulator

_Filed: 2026-10-21T21:37:52Z_
_Operation: service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 48 handlers in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and stores nothing, so no CodeArtifact client can do anything at all. Implement a FIRST minimal, fully stateful slice: the domain/repository lifecycle, which is the service's primary resource pair (`restJson1`, sigv4 name `codeartifact`).

### Happy path to implement (in order)

1. **CreateDomain** — `POST /v1/domain`. Input: `domain` (required, @httpQuery `domain`, len 2..50, pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`), `encryptionKey`, `tags`. Output: `domain` (DomainDescription) built from the newly stored record — `name`, `owner` (12-digit account id), `arn` (`arn:aws:codeartifact:<region>:<owner>:domain/<name>`), `status` = `Active`, `createdTime` (now), `encryptionKey` (echo input or a synthesized KMS key ARN), `repositoryCount` = 0, `assetSizeBytes` = 0, `s3BucketArn`. Creating a domain that already exists must fail `ConflictException` (HTTP 409).
2. **CreateRepository** — `POST /v1/repository`. Input: `domain` (required), `domainOwner`, `repository` (required, @httpQuery `repository`, len 2..100, pattern `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$`), `description`, `upstreams` (list<UpstreamRepository>), `tags`. Output: `repository` (RepositoryDescription) with `name`, `administratorAccount`, `domainName`, `domainOwner`, `arn` (`arn:aws:codeartifact:<region>:<owner>:repository/<domain>/<repo>`), `description`, `upstreams` (as UpstreamRepositoryInfoList, order preserved), `externalConnections` = [], `createdTime`. Unknown domain ⇒ `ResourceNotFoundException` (404); duplicate repository name in the domain ⇒ `ConflictException` (409). Must increment the owning domain's `repositoryCount`.
3. **DescribeRepository** — `GET /v1/repository`. Input: `domain` (required), `domainOwner`, `repository` (required). Output: the SAME `RepositoryDescription` that was stored by step 2 (same arn, description, upstreams, createdTime) — read from state, never re-synthesized from the request. Missing domain or repository ⇒ `ResourceNotFoundException` (404).
4. **ListRepositoriesInDomain** — `POST /v1/domain/repositories`. Input: `domain` (required), `domainOwner`, `administratorAccount`, `repositoryPrefix`, `maxResults` (1..1000), `nextToken`. Output: `repositories` (list<RepositorySummary>: `name`, `administratorAccount`, `domainName`, `domainOwner`, `arn`, `description`, `createdTime`) + `nextToken`. Must list exactly the repositories created in that domain, honour `repositoryPrefix` filtering and `maxResults`/`nextToken` paging (`@paginated(inputToken=nextToken, outputToken=nextToken, items=repositories)`), and omit `nextToken` on the last page.
5. **DeleteRepository** — `DELETE /v1/repository`. Input: `domain` (required), `domainOwner`, `repository` (required). Output: `repository` (RepositoryDescription) of the repository as it was just before removal. Must actually remove it from state (subsequent DescribeRepository ⇒ 404) and decrement the domain's `repositoryCount`. Unknown repository ⇒ `ResourceNotFoundException` (404).
6. **DeleteDomain** — `DELETE /v1/domain`. Input: `domain` (required), `domainOwner`. Output: `domain` (DomainDescription) of the removed domain. Deleting a domain that still contains repositories ⇒ `ResourceNotFoundException`/`ConflictException` per the model's declared errors (CodeArtifact requires the domain be empty); unknown domain ⇒ `ResourceNotFoundException` (404). After success, DescribeDomain and ListDomains must no longer show it.

Also wire **DescribeDomain** (`GET /v1/domain`) and **ListDomains** (`POST /v1/domains`, paginated on `domains` of DomainSummary) if cheap — they read the exact same records and make the slice verifiable, but the six operations above are the required minimum.

Every one of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`): no canned bodies, no echoing the input back as if it were stored, no random ARNs that are not saved. A create followed by a describe/list must return precisely what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs`, on the service state struct:

- `domains: BTreeMap<String /*domain name*/, Domain>` where
  `Domain { name: String, owner: String /*12-digit account*/, arn: String, status: DomainStatus /*Active*/, created_time: SystemTime, encryption_key: Option<String>, asset_size_bytes: i64, s3_bucket_arn: String, tags: Vec<Tag>, policy: Option<ResourcePolicy>, repositories: BTreeMap<String /*repo name*/, Repository> }`
  (`repositoryCount` is derived as `repositories.len()`, never stored stale).
- `Repository { name: String, domain_name: String, domain_owner: String, administrator_account: String, arn: String, description: Option<String>, upstreams: Vec<UpstreamRepositoryInfo>, external_connections: Vec<RepositoryExternalConnectionInfo>, created_time: SystemTime, tags: Vec<Tag>, policy: Option<ResourcePolicy> }`
  — later slices hang `packages: BTreeMap<(PackageFormat, Option<String> /*namespace*/, String /*package*/), Package>` off this for the package-version operations.
- Pagination tokens: opaque, encode the last-returned key (e.g. base64 of `repository name`) and resolve against the sorted `BTreeMap` keys so `nextToken` is stable across calls.

All other operations (package versions, package groups, permissions policies, tags, external connections, GetAuthorizationToken, GetRepositoryEndpoint) stay 501 for now and are tracked as separate gap issues.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/` matches all 48 operation files (create_domain.rs, create_repository.rs, describe_repository.rs, list_repositories_in_domain.rs, delete_repository.rs, delete_domain.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501. Contracts cited from the crate's OPERATIONS.md (routes, required members, @httpQuery bindings, length/pattern constraints, declared errors, @paginated traits) and from the Smithy model shapes DomainDescription, RepositoryDescription, RepositorySummary and DomainSummary (member lists quoted above). No tests assert any of this behavior yet (only the generated `tests/state_roundtrip.rs` example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI codeartifact client at it.
2. `aws codeartifact create-domain --domain my-domain` — expect 200 with domain.name=my-domain, status=Active, arn and createdTime populated, repositoryCount=0; actual: HTTP 501 NotImplemented.
3. `aws codeartifact create-repository --domain my-domain --repository my-repo --description 'demo'` — expect 200 with repository.domainName=my-domain, description='demo'.
4. `aws codeartifact describe-repository --domain my-domain --repository my-repo` — expect the identical RepositoryDescription (same arn, description, createdTime) served from state.
5. `aws codeartifact list-repositories-in-domain --domain my-domain --max-results 1` — expect exactly my-repo in `repositories`, prefix filter and paging honoured.
6. `aws codeartifact delete-domain --domain my-domain` while my-repo still exists — expect the modeled failure (409/404), not success.
7. `aws codeartifact delete-repository --domain my-domain --repository my-repo` then `describe-repository` — expect 200 returning the deleted repository, then ResourceNotFoundException (404).
8. `aws codeartifact delete-domain --domain my-domain` then `describe-domain` — expect 200 then ResourceNotFoundException (404).
