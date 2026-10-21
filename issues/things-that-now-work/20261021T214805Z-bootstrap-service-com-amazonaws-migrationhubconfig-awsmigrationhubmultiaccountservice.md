Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MigrationHub Config emulator

_Filed: 2026-10-21T21:48:05Z_
_Operation: service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `src/ops/` of service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, returning HTTP 501 `NotImplemented`) and the service holds no state. The service models exactly 4 operations, which together form its entire home-region-control lifecycle, so the first slice is all four implemented statefully over one shared store.

Happy path to implement, in order (wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: AWSMigrationHubMultiAccountService.<OperationName>`):

1. **CreateHomeRegionControl** — input `HomeRegion` (required, must match `^([a-z]+)-([a-z]+)-([0-9]+)$`, len 1..50), `Target` (required, `Target{Type: ACCOUNT (required enum), Id: 12-digit account id}`), `DryRun` (boolean, default false). On success persist a new control and return `HomeRegionControl{ControlId, HomeRegion, Target, RequestedTime}`. `ControlId` must be freshly generated and match `^hrc-[a-z0-9]{12}$`. When `DryRun` is true, validate and return the modeled `DryRunOperation` error (HTTP 400) and persist NOTHING. An account may have only one home region: a second create for the same target with a different `HomeRegion` must fail with `InvalidInputException` (HTTP 400), not silently create a duplicate.
2. **GetHomeRegion** — no input. Must read the stored control for the calling account and return its `HomeRegion`. Before any control exists, it must NOT return an empty/hollow 200 — return the modeled `InvalidInputException` (home region not set). After step 1, it must return exactly the `HomeRegion` that was created.
3. **DescribeHomeRegionControls** — input `ControlId` (`^hrc-[a-z0-9]{12}$`), `HomeRegion`, `Target`, `MaxResults` (1..100), `NextToken` (len 0..2048, `^[a-zA-Z0-9\/\+\=]{0,2048}$`). Output `HomeRegionControls` (list<HomeRegionControl>, len 0..100) plus `NextToken`. Must return the controls actually in the store, filtered by whichever of `ControlId`/`HomeRegion`/`Target` were supplied, truncated to `MaxResults` with an opaque `NextToken` emitted only when more results remain and honoured on the follow-up call. An unknown `ControlId` filter yields an empty list, not an error.
4. **DeleteHomeRegionControl** — input `ControlId` (required, `^hrc-[a-z0-9]{12}$`). Removes the control from the store; empty output body on success. A `ControlId` that is not present must fail with `InvalidInputException` (HTTP 400), and after a successful delete `DescribeHomeRegionControls` must no longer list it and `GetHomeRegion` must again report the region as unset.

Every one of the four must compute its response from persisted state reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back, no generating a `ControlId` that is never stored. Input constraints (length, pattern, enum, `MaxResults` range) must be enforced with `InvalidInputException` rather than accepted.

## State Model

In `src/state.rs`, on the service state struct:

- `home_region_controls: BTreeMap<String /* ControlId, "hrc-" + 12 lowercase alnum */, HomeRegionControlRecord>` — insertion/`RequestedTime` ordering preserved for stable pagination.
- `HomeRegionControlRecord { control_id: String, home_region: String, target_type: TargetType /* ACCOUNT */, target_id: Option<String> /* 12-digit account id */, requested_time: SystemTime }`.
- `home_region_by_account: BTreeMap<String /* 12-digit account id */, String /* ControlId */>` — enforces the one-home-region-per-account rule and backs `GetHomeRegion` (lookup by the caller's account; absent ⇒ region not set).
- `next_control_seq: u64` — monotonic counter feeding deterministic, unique `ControlId` generation.
- Pagination token: an opaque, base64-ish (`^[a-zA-Z0-9/+=]*$`) encoding of the last-returned `ControlId`, resolved against `home_region_controls` on the next call; an unparseable or unknown token ⇒ `InvalidInputException`.

Done when a client script can run CreateHomeRegionControl → GetHomeRegion → DescribeHomeRegionControls → DeleteHomeRegionControl → DescribeHomeRegionControls (now empty) end to end with correct bodies, and a DryRun create returns `DryRunOperation` while leaving state untouched.

## Evidence

OPERATIONS.md at the crate root lists all 4 modeled operations with their input/output members and errors: CreateHomeRegionControl (`HomeRegion`/`Target`/`DryRun` required; output `HomeRegionControl`; errors include `DryRunOperation`, `InvalidInputException`), GetHomeRegion (no input, output `HomeRegion`), DescribeHomeRegionControls (@paginated inputToken=NextToken/outputToken=NextToken, `MaxResults` 1..100, output `HomeRegionControls` list 0..100), DeleteHomeRegionControl (`ControlId` required, no output members). `smithy_query` on shapes `HomeRegionControl` and `Target` confirms the members `ControlId` (`^hrc-[a-z0-9]{12}$`), `HomeRegion`, `Target`, `RequestedTime` (timestamp) and `Target{Type: TargetType required, Id: ^\d{12}$ len 12}`, and documents that the target is always `ACCOUNT` and must be the current account. README.md documents that every generated stub answers HTTP 501 until the `SCAFFOLDED, NOT IMPLEMENTED` marker is deleted and real behaviour written, and that handlers must reach the single shared engine store via `crate::state::service_state(&*self.engine)`. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `migrationhub-config` client at it.
2. Call CreateHomeRegionControl with HomeRegion="us-west-2", Target={Type:"ACCOUNT", Id:"123456789012"}, DryRun=false — expect 200 with HomeRegionControl{ControlId matching ^hrc-[a-z0-9]{12}$, HomeRegion="us-west-2", Target echoed, RequestedTime set}; currently 501 NotImplemented.
3. Call GetHomeRegion with an empty body — expect 200 with HomeRegion="us-west-2" read from state.
4. Call DescribeHomeRegionControls with no filters — expect the one created control in HomeRegionControls; repeat with ControlId=<created id> and with HomeRegion="us-west-2" and expect the same single item; with HomeRegion="eu-central-1" expect an empty list.
5. Call CreateHomeRegionControl again with HomeRegion="eu-central-1" for the same account — expect InvalidInputException (HTTP 400), and GetHomeRegion still reports "us-west-2".
6. Call CreateHomeRegionControl with DryRun=true — expect DryRunOperation (HTTP 400) and no new control in DescribeHomeRegionControls.
7. Call DeleteHomeRegionControl with the created ControlId — expect 200 with an empty body; then DescribeHomeRegionControls returns an empty list and GetHomeRegion reports the region as unset (InvalidInputException).
8. Call DeleteHomeRegionControl again with the same ControlId — expect InvalidInputException (HTTP 400).
