Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Service Catalog emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-servicecatalog-aws242servicecatalogservice | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 90 operations in `service-com-amazonaws-servicecatalog-aws242servicecatalogservice` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST minimal slice: the stateful lifecycle of the primary resource, the **portfolio**, so a realistic client script works end to end over awsJson1_1 (`POST /`, `X-Amz-Target: AWS242ServiceCatalogService.<Op>`).

### Happy path (in order)

1. **CreatePortfolio** — input `DisplayName` (required, 1..100), `ProviderName` (required, 1..50), `Description` (0..2000), `Tags` (list<Tag{Key,Value}>, 0..20), `IdempotencyToken` (required), `AcceptLanguage`. Must mint a new portfolio id matching `^[a-zA-Z0-9_\-]*$` (AWS form `port-<12 chars>`), an `ARN` (`arn:aws:catalog:<region>:<account>:portfolio/<id>`) and a `CreatedTime`, persist the record, and return `PortfolioDetail{Id, ARN, DisplayName, Description, ProviderName, CreatedTime}` plus the stored `Tags`. Repeating the call with the SAME `IdempotencyToken` and the same other inputs must return the SAME portfolio rather than creating a second one (`@idempotencyToken`).
2. **DescribePortfolio** — input `Id` (required). Must look the portfolio up in state and return the exact `PortfolioDetail` and `Tags` that were stored by CreatePortfolio/UpdatePortfolio (plus empty `TagOptions`/`Budgets` lists). Unknown id -> `ResourceNotFoundException` (HTTP 400), never a hollow 200.
3. **ListPortfolios** — input `PageSize` (0..100), `PageToken`. Must return `PortfolioDetails` containing every persisted portfolio (one entry per created, non-deleted portfolio), honouring `PageSize` as a real bound and emitting `NextPageToken` only when more remain; a token handed back must resume where the previous page stopped. An invalid/unknown token -> `InvalidParametersException`.
4. **UpdatePortfolio** — input `Id` (required), optional `DisplayName`, `Description`, `ProviderName`, `AddTags`, `RemoveTags`. Must mutate the STORED record in place: only supplied fields change, `AddTags` merges (overwriting an existing key's value), `RemoveTags` deletes by key, and the response `PortfolioDetail`/`Tags` must be the post-update stored state, observable by a subsequent DescribePortfolio and ListPortfolios. Unknown id -> `ResourceNotFoundException`.
5. **DeletePortfolio** — input `Id` (required). Must remove the record from state; afterwards DescribePortfolio returns `ResourceNotFoundException` and ListPortfolios no longer lists it. Deleting an unknown id -> `ResourceNotFoundException`.

No canned, echoed or hardcoded responses: every field in every response above must be computed from persisted state, and a random id that is not stored is a failure. Ids/ARNs/timestamps are generated once at create time and then read back verbatim.

## State Model

In `src/state.rs` (`ComAmazonawsServicecatalogAws242servicecatalogserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `portfolios: BTreeMap<String /* portfolio id, e.g. "port-abc123def456" */, Portfolio>` — BTreeMap so ListPortfolios/pagination has a stable deterministic order.
- `struct Portfolio { id: String, arn: String, display_name: String, description: Option<String>, provider_name: String, created_time: SystemTime, tags: BTreeMap<String /*Key*/, String /*Value*/> }`
- `idempotency: HashMap<String /*IdempotencyToken*/, String /*portfolio id*/>` — backs CreatePortfolio replay.
- `next_id_seed: u64` (or any deterministic id generator) used to mint `port-…` ids.

Everything else in the service (products, provisioning artifacts, ProvisionProduct, shares, constraints, service actions, tag options) stays scaffolded at 501 and will be filed as individual gap issues later.

## Done when

An integration test drives Create -> Describe -> List -> Update -> Describe -> Delete -> Describe(404-style `ResourceNotFoundException`) and asserts the response BODIES, not just 2xx.

## Evidence

Spec: `services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json`. OPERATIONS.md (crate root) documents the contracts used above: CreatePortfolio (line 87-91: required `DisplayName`/`ProviderName`/`IdempotencyToken`; output `PortfolioDetail` + `Tags`), DescribePortfolio (189-193: input `Id`, output `PortfolioDetail`/`Tags`/`TagOptions`/`Budgets`, error `ResourceNotFoundException`), ListPortfolios (389-394: `@paginated(inputToken=PageToken, outputToken=NextPageToken)`, `PageSize` range 0..100, output `PortfolioDetails` + `NextPageToken`), UpdatePortfolio (538-542: `AddTags`/`RemoveTags`, `ResourceNotFoundException`), DeletePortfolio (135-139: `ResourceInUseException`/`ResourceNotFoundException`). `smithy_query shape=PortfolioDetail` gives the exact members `Id, ARN, DisplayName, Description, CreatedTime, ProviderName`; `Tag` requires `Key` and `Value`. All-stub status confirmed by `src/ops/create_portfolio.rs` lines 4 and 171: `//! SCAFFOLDED, NOT IMPLEMENTED:` and `Err(crate::scaffolded::Scaffolded::not_implemented("CreatePortfolio"))`; all 89 other op files carry the same marker. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI servicecatalog client at it.
2. `aws servicecatalog create-portfolio --display-name demo --provider-name acme --idempotency-token tok-1 --tags Key=env,Value=dev` — expect 200 with `PortfolioDetail.Id` of the form `port-…`, an ARN and a CreatedTime; today: HTTP 501 NotImplemented.
3. Repeat the identical create with `--idempotency-token tok-1` — expect the SAME portfolio id returned, not a second portfolio.
4. `aws servicecatalog describe-portfolio --id <id>` — expect the DisplayName/ProviderName/Description/Tags exactly as created.
5. `aws servicecatalog list-portfolios --page-size 1` — expect the created portfolio listed and a `NextPageToken` only if more portfolios exist; resume with `--page-token` and expect the next page.
6. `aws servicecatalog update-portfolio --id <id> --description updated --add-tags Key=team,Value=platform --remove-tags env`, then describe again — expect description `updated`, tag `team=platform` present and `env` gone.
7. `aws servicecatalog delete-portfolio --id <id>`, then `describe-portfolio --id <id>` — expect `ResourceNotFoundException`, and the portfolio absent from `list-portfolios`.
8. `aws servicecatalog describe-portfolio --id port-doesnotexist` — expect `ResourceNotFoundException`, not 501 and not an empty 200.
