Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the ssm emulator

_Filed: 2026-10-21T23:23:19Z_
_Operation: service-com-amazonaws-ssm-amazonssm | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-ssm-amazonssm/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the crate holds no state. Nothing in the service works end-to-end.

This issue defines the FIRST, SMALL slice to implement: the Parameter Store lifecycle, which is by far the most-used part of SSM (clients store config/secrets with `PutParameter` and read them with `GetParameter`/`GetParametersByPath`). Operations outside this slice stay stubbed and will be filed as individual gap issues later.

### Happy path to implement (in order)

1. **PutParameter** — input `Name` (required, 1..2048), `Value` (required), `Type` (`String` | `StringList` | `SecureString`), `Description`, `KeyId`, `Overwrite` (default false), `AllowedPattern`, `Tier`, `DataType`, `Tags`. Output `Version` (long, required) and `Tier`. Must persist the parameter and allocate a version: first write => `Version: 1`; a write to an existing name with `Overwrite=true` => previous version + 1 and the new value replaces the current one; a write to an existing name WITHOUT `Overwrite` => `ParameterAlreadyExists` (HTTP 400). `Type` defaults must be stored, not echoed back from thin air.
2. **GetParameter** — input `Name` (required), `WithDecryption`. Output `Parameter` built from stored state: `Name`, `Type`, `Value`, `Version`, `LastModifiedDate`, `ARN` (`arn:aws:ssm:<region>:<account>:parameter/<name-without-leading-slash>`), `DataType`. Unknown name => `ParameterNotFound` (HTTP 400). Must return the value most recently written by `PutParameter`, including after an overwrite.
3. **GetParameters** — input `Names` (required, 1..10), `WithDecryption`. Output `Parameters` (one `Parameter` per name that exists, same fields as above) and `InvalidParameters` (the names that do not exist). Must NOT fail the whole call for a missing name.
4. **GetParametersByPath** — input `Path` (required), `Recursive` (default false), `MaxResults` (1..10), `NextToken`. Output `Parameters` + `NextToken`. Must select from stored state by hierarchy prefix: with `Recursive=false` return only parameters one level below `Path`; with `Recursive=true` return the whole subtree. `MaxResults` must truncate and emit an opaque `NextToken` that resumes exactly where the previous page stopped (no duplicates, no skips); an unrecognised token => `InvalidNextToken` (HTTP 400).
5. **DescribeParameters** — input `MaxResults` (1..50), `NextToken`, `ParameterFilters`. Output `Parameters` as `ParameterMetadata` (`Name`, `ARN`, `Type`, `KeyId`, `LastModifiedDate`, `Description`, `AllowedPattern`, `Version`, `Tier`, `DataType`) — metadata only, NO `Value` — plus `NextToken`. Must list exactly the parameters currently stored. Minimum filter support: `ParameterFilters` with `Key=Name`/`Key=Type` and `Option=Equals|BeginsWith`.
6. **DeleteParameter** — input `Name` (required). Empty output. Must remove the parameter (and its version history) from state; unknown name => `ParameterNotFound` (HTTP 400). A subsequent `GetParameter` for the same name must return `ParameterNotFound`, and `DescribeParameters` must no longer list it.

### Requirements

- Every one of the six operations must compute its response from the shared, persisted store reached via `crate::state::service_state(&*self.engine)` (see README "State"). NO canned responses, NO echoing the input back, NO generating a version number without storing the parameter.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the six handler files as they are implemented.
- Modeled errors must be returned as the modeled shapes with their documented HTTP status: `ParameterAlreadyExists`, `ParameterNotFound`, `InvalidNextToken` (all HTTP 400).
- `SecureString` values may be stored in plaintext internally, but `GetParameter`/`GetParameters` must still round-trip the exact value written.
- Done = an integration script running the steps below passes with correct, stateful bodies — not merely 2xx.

## State Model

In `src/state.rs`, on `ComAmazonawsSsmAmazonssmState`:

- `parameters: BTreeMap<String, ParameterEntry>` — keyed by the full normalised parameter name (leading `/` preserved; `BTreeMap` so `GetParametersByPath`/`DescribeParameters` iterate in a stable, prefix-friendly order).
- `ParameterEntry { name: String, arn: String, type_: ParameterType, value: String, version: i64, description: Option<String>, key_id: Option<String>, allowed_pattern: Option<String>, tier: ParameterTier, data_type: String /* default "text" */, policies: Option<String>, last_modified_date: SystemTime, last_modified_user: String, tags: Vec<Tag>, history: Vec<ParameterVersion> }`.
- `ParameterVersion { version: i64, value: String, type_: ParameterType, labels: Vec<String>, last_modified_date: SystemTime, description: Option<String>, data_type: String }` — appended on every successful `PutParameter` so a later `GetParameterHistory`/`LabelParameterVersion` slice has real data to read.
- Page-token helper: a `NextToken` encoding the last-returned parameter name (plus the request's filter/path fingerprint), so paging resumes deterministically for both `GetParametersByPath` and `DescribeParameters`.

## Evidence

Census/grep: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-ssm-amazonssm/src/ops/` matches every operation file, including `put_parameter.rs`, `get_parameter.rs`, `get_parameters.rs`, `get_parameters_by_path.rs`, `describe_parameters.rs`, `delete_parameter.rs`; all return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. Contract fields cited from `services/service-com-amazonaws-ssm-amazonssm/OPERATIONS.md`: PutParameter (line 775-779: required `Name`/`Value`, output required `Version` + `Tier`, errors incl. `ParameterAlreadyExists`), GetParameter (570-574: output `Parameter`, errors `ParameterNotFound`), GetParameters (583-587: `Names` 1..10, output `Parameters` + `InvalidParameters`), GetParametersByPath (589-594: `Path` required, `Recursive`, `MaxResults` 1..10, `NextToken`, `@paginated`, error `InvalidNextToken`), DescribeParameters (399-404: `MaxResults` 1..50, output `Parameters` as `ParameterMetadata` + `NextToken`), DeleteParameter (159-163: `Name` required, empty output, `ParameterNotFound`). Shape members for `Parameter` (Name/Type/Value/Version/Selector/LastModifiedDate/ARN/DataType) and `ParameterMetadata` (no `Value` member) come from the Smithy model via smithy_query. Version-increment-on-overwrite and `ParameterAlreadyExists` without `Overwrite` are well-established Parameter Store semantics. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ssm` client at it.
2. `aws ssm put-parameter --name /app/db/url --value postgres://localhost/app --type String` => expect HTTP 200 with `Version: 1`; actual: HTTP 501 NotImplemented.
3. `aws ssm put-parameter --name /app/db/password --value s3cr3t --type SecureString` => expect `Version: 1`.
4. `aws ssm get-parameter --name /app/db/url` => expect `Parameter.Value == postgres://localhost/app`, `Version == 1`, populated `ARN` and `LastModifiedDate`.
5. `aws ssm put-parameter --name /app/db/url --value postgres://db/app --type String --overwrite` => expect `Version: 2`; then `get-parameter --name /app/db/url` must return the NEW value with `Version: 2`.
6. `aws ssm put-parameter --name /app/db/url --value x --type String` (no --overwrite) => expect `ParameterAlreadyExists` with HTTP 400.
7. `aws ssm get-parameters --names /app/db/url /app/db/password /app/missing` => expect two entries in `Parameters` and `/app/missing` in `InvalidParameters`.
8. `aws ssm get-parameters-by-path --path /app/db --recursive` => expect both stored parameters; repeat with `--max-results 1` and follow `NextToken` => expect the two parameters across two pages with no duplicates.
9. `aws ssm describe-parameters` => expect both parameters as metadata (no `Value` field present).
10. `aws ssm delete-parameter --name /app/db/url` => expect HTTP 200; then `get-parameter --name /app/db/url` => expect `ParameterNotFound` (400), and `describe-parameters` must no longer list it.
11. `aws ssm delete-parameter --name /app/db/url` again => expect `ParameterNotFound` (400).
