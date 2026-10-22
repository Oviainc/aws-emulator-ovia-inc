Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the s3control emulator

_Filed: 2026-10-21T21:28:19Z_
_Operation: service-com-amazonaws-s3control-awss3controlservicev20180820 | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). No client workflow can succeed.

Implement a FIRST SMALL SLICE: the access point lifecycle, which is S3 Control's primary resource. All five operations must compute their responses from the shared per-service state reached via `crate::state::service_state(&*self.engine)` — no canned, echoed or hardcoded responses, and no random ARN without storage. Delete the scaffold marker line in each file as it is implemented.

Happy path, in order (routes and fields from OPERATIONS.md):

1. **CreateAccessPoint** — `PUT /v20180820/accesspoint/{Name}`. Input: `AccountId` (required, `x-amz-account-id` header, `^\d{12}$`), `Name` (httpLabel, len 3..255, required), `Bucket` (required), optional `VpcConfiguration`, `PublicAccessBlockConfiguration`, `BucketAccountId`, `Tags`. Must persist a new access point keyed by (AccountId, Name) and return the computed `AccessPointArn` (`arn:aws:s3:<region>:<AccountId>:accesspoint/<Name>`) and `Alias`. `NetworkOrigin` must be derived: `VPC` when `VpcConfiguration.VpcId` is present, otherwise `Internet`. A repeat create of the same name must not silently duplicate the entry (real S3 Control rejects it with `AccessPointAlreadyOwnedByYou`, 409).
2. **GetAccessPoint** — `GET /v20180820/accesspoint/{Name}`. Must return the STORED record: `Name`, `Bucket`, `NetworkOrigin`, `VpcConfiguration`, `PublicAccessBlockConfiguration`, `CreationDate` (the timestamp recorded at create), `Alias`, `AccessPointArn`, `BucketAccountId`. An unknown name must fail as not-found (S3-style error, HTTP 404 `NoSuchAccessPoint`) rather than returning an empty 200.
3. **ListAccessPoints** — `GET /v20180820/accesspoint`. Input: `AccountId`, optional `Bucket` filter (`?bucket=`), `MaxResults` (`?maxResults=`, range 0..1000), `NextToken` (`?nextToken=`). Must return `AccessPointList` built from stored state for that account, filtered by `Bucket` when supplied, truncated to `MaxResults` with a `NextToken` that resumes the scan on the next call (the op is `@paginated`).
4. **PutAccessPointPolicy** — `PUT /v20180820/accesspoint/{Name}/policy`. Input: `AccountId`, `Name`, `Policy` (required). Must store the policy document on the named access point; unknown name is not-found.
5. **GetAccessPointPolicy** — `GET /v20180820/accesspoint/{Name}/policy`. Must return the exact `Policy` string previously stored by PutAccessPointPolicy (byte-for-byte), and fail not-found (`NoSuchAccessPointPolicy`) when no policy was ever put.
6. **DeleteAccessPoint** — `DELETE /v20180820/accesspoint/{Name}`. Must remove the record (and its policy) so a subsequent GetAccessPoint is not-found and ListAccessPoints no longer contains it. Deleting an unknown name is not-found.

## State Model

Add to `ComAmazonawsS3controlAwss3controlservicev20180820State`:

- `access_points: HashMap<(String /*account_id*/, String /*name*/), AccessPointRecord>`
- `AccessPointRecord { name: String, account_id: String, bucket: String, bucket_account_id: Option<String>, arn: String, alias: String, network_origin: NetworkOrigin /* Internet | VPC */, vpc_configuration: Option<VpcConfiguration>, public_access_block_configuration: Option<PublicAccessBlockConfiguration>, creation_date: DateTime, tags: Vec<Tag>, policy: Option<String> }`

Insertion order (or a `Vec<(account, name)>` index) must be preserved so ListAccessPoints paging is stable across calls; `NextToken` can be the opaque encoding of the last-returned name.

Everything else in the service (Jobs, Access Grants, Multi-Region Access Points, Storage Lens, bucket sub-resources, Object Lambda access points, public access block) stays 501 for now and is covered by separate per-operation gap issues.

Done = an integration test that drives the six calls above against one running emulator passes with correct, stateful bodies — not merely 2xx.

## Evidence

OPERATIONS.md in the crate root defines the access point lifecycle used above: `CreateAccessPoint` (`PUT /v20180820/accesspoint/{Name}`, input AccountId/Name/Bucket/VpcConfiguration/PublicAccessBlockConfiguration/BucketAccountId/Tags, output AccessPointArn+Alias, line 54), `DeleteAccessPoint` (line 124), `GetAccessPoint` (output Name/Bucket/NetworkOrigin/VpcConfiguration/PublicAccessBlockConfiguration/CreationDate/Alias/AccessPointArn/BucketAccountId, line 292), `GetAccessPointPolicy` (output Policy, line 313), `ListAccessPoints` (`@paginated(inputToken=NextToken, outputToken=NextToken)`, Bucket/MaxResults/NextToken query params, output AccessPointList+NextToken, line 484), `PutAccessPointPolicy` (Policy required, line 577). Stub status: `src/ops/create_access_point.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and the same marker is present in every file under `src/ops/`; `src/state.rs:8` is an empty struct with a TODO, so no state exists to serve reads. Not-found semantics and `AccessPointAlreadyOwnedByYou` are well-established S3 Control behavior (the model declares no per-operation errors, so these surface as S3-style XML errors).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI s3control client at it with account id 123456789012.
2. CreateAccessPoint: `aws s3control create-access-point --account-id 123456789012 --name my-ap --bucket my-bucket` -> expect 200 with AccessPointArn `arn:aws:s3:<region>:123456789012:accesspoint/my-ap` and an Alias; actual: HTTP 501 NotImplemented.
3. GetAccessPoint --account-id 123456789012 --name my-ap -> expect Bucket=my-bucket, NetworkOrigin=Internet, a CreationDate matching the create, and the same ARN.
4. ListAccessPoints --account-id 123456789012 --bucket my-bucket -> expect AccessPointList containing exactly my-ap; repeat with --max-results 1 over two access points and follow NextToken to see each once.
5. PutAccessPointPolicy --account-id 123456789012 --name my-ap --policy '{"Version":"2012-10-17","Statement":[]}' then GetAccessPointPolicy -> expect the identical policy string back.
6. DeleteAccessPoint --account-id 123456789012 --name my-ap, then GetAccessPoint for the same name -> expect a 404 NoSuchAccessPoint and ListAccessPoints to return an empty list.
