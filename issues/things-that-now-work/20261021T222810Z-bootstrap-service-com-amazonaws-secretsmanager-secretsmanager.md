Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the Secrets Manager emulator

_Filed: 2026-10-21T22:28:10Z_
_Operation: service-com-amazonaws-secretsmanager-secretsmanager | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), so the service holds no state at all. Implement a FIRST, SMALL stateful slice: the secret lifecycle loop.

Happy path to implement, in order (protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: secretsmanager.<OperationName>`):

1. **CreateSecret** — input `Name` (required), `Description`, `KmsKeyId`, `SecretString` XOR `SecretBinary`, `Tags`, `ClientRequestToken` (@idempotencyToken). Must allocate the ARN as `arn:aws:secretsmanager:<region>:<account>:secret:<Name>-<6 random chars>`, create version 1 with a fresh UUID `VersionId` (use `ClientRequestToken` when supplied) and stage it `AWSCURRENT`. Returns `ARN`, `Name`, `VersionId` computed from what was just stored. A second CreateSecret with the same `Name` must fail `ResourceExistsException` (HTTP 400); same `Name` + same `ClientRequestToken` + identical value is idempotent.
2. **GetSecretValue** — input `SecretId` (name OR ARN), optional `VersionId`, optional `VersionStage` (default `AWSCURRENT`). Returns `ARN`, `Name`, `VersionId`, the stored `SecretString`/`SecretBinary` byte-for-byte, `VersionStages`, `CreatedDate`. Unknown `SecretId` or a stage with no version → `ResourceNotFoundException`; a secret scheduled for deletion → `InvalidRequestException`.
3. **PutSecretValue** — input `SecretId` (required), `SecretString`/`SecretBinary`, `VersionStages` (default `[AWSCURRENT]`), `ClientRequestToken`. Must append a NEW version, move `AWSCURRENT` to it and demote the previous `AWSCURRENT` to `AWSPREVIOUS`. Returns `ARN`, `Name`, `VersionId`, `VersionStages`. A following GetSecretValue (no VersionId) must return the NEW value; `VersionStage=AWSPREVIOUS` must return the old one.
4. **DescribeSecret** — input `SecretId`. Returns from stored metadata: `ARN`, `Name`, `Description`, `KmsKeyId`, `Tags`, `CreatedDate`, `LastChangedDate`, `LastAccessedDate`, `DeletedDate` (only when scheduled for deletion), `RotationEnabled` (false), and `VersionIdsToStages` reflecting every live version and its stage labels.
5. **ListSecrets** — returns `SecretList` of `SecretListEntry` built from stored secrets (`ARN`, `Name`, `Description`, `KmsKeyId`, `Tags`, `CreatedDate`, `LastChangedDate`, `SecretVersionsToStages`). Honour `MaxResults` (1..100) with an opaque `NextToken` that resumes where the previous page ended, and exclude secrets scheduled for deletion unless `IncludePlannedDeletion` is true.
6. **DeleteSecret** — input `SecretId`, `RecoveryWindowInDays` (default 30, valid 7..30), `ForceDeleteWithoutRecovery`. Default marks the secret scheduled-for-deletion with `DeletionDate = now + window` and returns `ARN`, `Name`, `DeletionDate`; the secret then disappears from ListSecrets and GetSecretValue fails `InvalidRequestException`, while DescribeSecret still reports it with `DeletedDate`. `ForceDeleteWithoutRecovery=true` removes it outright so every later read is `ResourceNotFoundException`. Passing both `RecoveryWindowInDays` and `ForceDeleteWithoutRecovery` → `InvalidParameterException`.

Every one of these MUST compute its response from persisted state. No canned bodies, no echoing the input back, no random ARN/VersionId that is never stored. A CreateSecret followed by GetSecretValue/DescribeSecret in a separate request must return exactly what was created; SecretId lookups must accept both the bare name and the full ARN.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` (the guard shown in each stub's header docs):

```
SecretsManagerState {
    // keyed by secret Name; a parallel index maps full ARN -> Name so SecretId
    // accepts either form
    secrets: HashMap<String, Secret>,
    arn_index: HashMap<String, String>,
}

Secret {
    name: String,
    arn: String,                       // arn:aws:secretsmanager:<region>:<account>:secret:<name>-<6 chars>
    description: Option<String>,
    kms_key_id: Option<String>,
    tags: Vec<Tag>,                    // Key/Value
    created_date: SystemTime,
    last_changed_date: SystemTime,
    last_accessed_date: Option<SystemTime>,
    deleted_date: Option<SystemTime>,  // Some(..) => scheduled for deletion
    rotation_enabled: bool,            // false for the first slice
    versions: BTreeMap<String, SecretVersion>,  // VersionId -> version, insertion ordered
}

SecretVersion {
    version_id: String,                // UUID, or the caller's ClientRequestToken
    secret_string: Option<String>,
    secret_binary: Option<Vec<u8>>,    // SecretString XOR SecretBinary
    version_stages: Vec<String>,       // e.g. ["AWSCURRENT"] / ["AWSPREVIOUS"]
    created_date: SystemTime,
}
```

`VersionIdsToStages` in DescribeSecret and `SecretVersionsToStages` in ListSecrets are derived by walking `versions`. Stage movement (AWSCURRENT -> AWSPREVIOUS) is a mutation of `version_stages` inside a single lock so GetSecretValue never observes two AWSCURRENT versions.

All operations outside this list (rotation, replication, resource policies, tagging, BatchGetSecretValue, GetRandomPassword, RestoreSecret, UpdateSecret, UpdateSecretVersionStage, ValidateResourcePolicy) stay at 501 and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-secretsmanager-secretsmanager/OPERATIONS.md` lists all 23 operations with input/output members and errors: CreateSecret (lines 40-44: `Name` required, `SecretString`/`SecretBinary`, `ClientRequestToken` @idempotencyToken, out `ARN`/`Name`/`VersionId`, errors incl. `ResourceExistsException`), GetSecretValue (76-80: `SecretId` required, `VersionId`, `VersionStage`; out `SecretString`/`SecretBinary`/`VersionStages`/`CreatedDate`), PutSecretValue (102-106: `VersionStages`, out `VersionId`/`VersionStages`), DescribeSecret (58-62: out `VersionIdsToStages`, `DeletedDate`, `Tags`, `CreatedDate`, `RotationEnabled`), ListSecrets (82-87: @paginated with `MaxResults` 1..100 / `NextToken`, `IncludePlannedDeletion`), DeleteSecret (52-56: `RecoveryWindowInDays`, `ForceDeleteWithoutRecovery`, out `DeletionDate`). Wire protocol `awsJson1_1`, target prefix `secretsmanager.` (OPERATIONS.md lines 8-15). Stub confirmation: `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/create_secret.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 281 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateSecret"))`; all 23 op files in `src/ops/` are the same generated shape. The AWSCURRENT/AWSPREVIOUS staging-label behaviour and the 7-30 day recovery window are documented Secrets Manager semantics (also described in the CreateSecret stub's own modeled-contract docs, lines 60-64).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with `--endpoint-url`.
2. `aws secretsmanager create-secret --name demo/db --description 'demo' --secret-string '{"user":"admin","pass":"p1"}'` → expect 200 with an `ARN` ending in `demo/db-<6 chars>`, a `Name` of `demo/db` and a `VersionId`. Today: HTTP 501 NotImplemented.
3. Repeat the same create-secret → expect HTTP 400 `ResourceExistsException`.
4. `aws secretsmanager get-secret-value --secret-id demo/db` → expect `SecretString` exactly `{"user":"admin","pass":"p1"}`, `VersionStages: ["AWSCURRENT"]` and the VersionId from step 2.
5. `aws secretsmanager put-secret-value --secret-id demo/db --secret-string '{"user":"admin","pass":"p2"}'` → expect a new VersionId with `AWSCURRENT`.
6. `aws secretsmanager get-secret-value --secret-id demo/db` → expect `pass":"p2`; then `--version-stage AWSPREVIOUS` → expect `pass":"p1`.
7. `aws secretsmanager describe-secret --secret-id <full ARN from step 2>` → expect `VersionIdsToStages` with both version IDs mapped to `AWSCURRENT` and `AWSPREVIOUS`, plus the stored `Description` and `CreatedDate`.
8. `aws secretsmanager list-secrets` → expect exactly the one `demo/db` entry; with `--max-results 1` over two secrets expect a `NextToken` whose second page returns the remaining secret and no duplicates.
9. `aws secretsmanager delete-secret --secret-id demo/db --recovery-window-in-days 7` → expect `DeletionDate` ≈ now+7d; then `list-secrets` omits it, `get-secret-value` fails `InvalidRequestException`, and `describe-secret` still shows it with `DeletedDate`.
10. `aws secretsmanager delete-secret --secret-id demo/db --force-delete-without-recovery` on a fresh secret → every later `get-secret-value`/`describe-secret` fails `ResourceNotFoundException`.
