Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9010f89773c9fbf121fd2271b9167862206a1831

# Bootstrap the ssm (AmazonSSM) emulator: the Parameter Store label/batch-delete slice of the parameter lifecycle

_Filed: 2026-11-02T02:22:01Z_
_Operation: service-com-amazonaws-ssm-amazonssm | Class: stub_

> _Note: this issue was identified against the code base at commit `9010f89773c9fbf121fd2271b9167862206a1831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Parameter Store write/read/delete core of this crate is already written (`src/ops/put_parameter.rs`, `get_parameter.rs`, `get_parameters.rs`, `get_parameters_by_path.rs`, `get_parameter_history.rs`, `describe_parameters.rs`, `delete_parameter.rs` carry no `//! SCAFFOLDED, NOT IMPLEMENTED:` marker), but the parameter-label half of that same lifecycle and its batch delete are still scaffolds answering HTTP 501. The state record even carries the field they are supposed to write and says so: `ParameterVersionRecord.labels` is documented as "`LabelParameterVersion` is what writes them and is still a scaffold, so the set is empty today" (`src/state.rs:383-387`), while the read path `ParameterRecord::select` already resolves a non-numeric `name:selector` against `held.labels` (`src/state.rs:528-537`). So today every `name:label` read can only ever miss.

First slice — implement these THREE handlers so the label workflow closes end to end on top of the existing put/get/delete:

1. `LabelParameterVersion` (`src/ops/label_parameter_version.rs`) — input `Name` (required), `ParameterVersion` (long, optional: "If no version is specified, the system attaches the label to the latest version"), `Labels` (list 1..10, required); output `ParameterVersion` (required) and `InvalidLabels` for labels that fail the documented label requirements. It must INSERT the accepted labels into the chosen `ParameterVersionRecord.labels` set in the shared store, move a label already held by another version of the same parameter ("You can't attach the same label to different versions of the same parameter ... You can move a label from one version of a parameter to another"), refuse an unknown name with `ParameterNotFound` and an unknown version with `ParameterVersionNotFound`, and refuse a version that would exceed the documented per-version label maximum with `ParameterVersionLabelLimitExceeded`.
2. `UnlabelParameterVersion` (`src/ops/unlabel_parameter_version.rs`) — input `Name`, `ParameterVersion` (required), `Labels` (1..10); output `RemovedLabels` (the labels actually taken off that version, computed from the stored set) and `InvalidLabels` (those not attached to it). It must REMOVE them from the same `labels` set, so a subsequent `name:label` read misses again.
3. `DeleteParameters` (`src/ops/delete_parameters.rs`) — input `Names` (list 1..10, required); output `DeletedParameters` and `InvalidParameters`, both partitioned from what the store actually held: every name removed from the parameter map goes in `DeletedParameters`, every name that named nothing goes in `InvalidParameters`. The operation models no per-name error, so the partition is the only signal and must come from state, not be echoed.

Every one of the three must compute its response from the persisted state reached through `crate::state::service_state(&*self.engine)` — no canned `InvalidLabels: []`, no echoing the request's `Labels` back as `RemovedLabels`, no success for a name the store does not hold. After this slice, `GetParameter`/`GetParameters`/`GetParametersByPath` must observe the labels without any change to those files, since they already read `ParameterVersionRecord.labels`.

## State Model

All three reuse the structures already in `src/state.rs` — no new store:
- the parameter map keyed by trimmed parameter name -> `ParameterRecord { name, versions: Vec<ParameterVersionRecord>, .. }`, which `DeleteParameters` removes entries from (one entry per name, each removal reported in `DeletedParameters`).
- `ParameterVersionRecord { version: i64, value, type_, data_type, tier, description, allowed_pattern, key_id, policies, last_modified_date, labels: BTreeSet<String> }` (`src/state.rs:375-388`) — `labels` is the set `LabelParameterVersion` inserts into and `UnlabelParameterVersion` erases from; the label -> version mapping is derived from these per-version sets (a label appears in at most one version's set of a given parameter, which is how the "can't attach the same label to two versions" rule and the move are both represented).
- resolution stays `ParameterRecord::select(selector)` (`src/state.rs:528-537`): all digits -> version number, anything else -> a label looked up in `labels`.

Every other SSM operation (associations, documents, maintenance windows, OpsItems, patch baselines, sessions, commands, inventory, service settings, tagging) stays a 501 scaffold in this slice and is covered by its own per-operation issues.

## Evidence

OPERATIONS.md (`services/service-com-amazonaws-ssm-amazonssm/OPERATIONS.md`) fixes the fields of the slice:
- line 621-625: "## LabelParameterVersion — **Input:** `Name` (string, len 1..2048, required), `ParameterVersion` (long, default 0), `Labels` (list<string>, len 1..10, required) — **Output:** `InvalidLabels` (list<string>, len 1..10), `ParameterVersion` (long, default 0, required) — **Errors:** `InternalServerError (HTTP 500)`, `ParameterNotFound (HTTP 400)`, `ParameterVersionLabelLimitExceeded (HTTP 400)`, `ParameterVersionNotFound (HTTP 400)`, `TooManyUpdates (HTTP 400)`"
- line 889-893: "## UnlabelParameterVersion — **Input:** `Name` ..., `ParameterVersion` (long, default 0, required), `Labels` (list<string>, len 1..10, required) — **Output:** `RemovedLabels` (list<string>, len 1..10), `InvalidLabels` (list<string>, len 1..10)"
- line 165-169: "## DeleteParameters — **Input:** `Names` (list<string>, len 1..10, required) — **Output:** `DeletedParameters` (list<string>, len 1..10), `InvalidParameters` (list<string>, len 1..10) — **Errors:** `InternalServerError (HTTP 500)`"
- line 10-11: "**Protocol:** `awsJson1_1` ... `POST /` with header `X-Amz-Target: AmazonSSM.<OperationName>`"

The handlers are unwritten scaffolds:
- `src/ops/label_parameter_version.rs:4`: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and"; `:191`: `Err(crate::scaffolded::Scaffolded::not_implemented("LabelParameterVersion"))`
- `src/ops/unlabel_parameter_version.rs:4` (same marker); `:172`: `Err(crate::scaffolded::Scaffolded::not_implemented("UnlabelParameterVersion"))`
- `src/ops/delete_parameters.rs:4` (same marker)

The crate itself names the hole and the field to fill:
- `src/state.rs:383-387`: "/// The labels that point at THIS version. `LabelParameterVersion` is what writes / them and is still a scaffold, so the set is empty today — but a `name:label` / selector resolves against it rather than against anything invented ... pub labels: BTreeSet<String>,"
- `src/state.rs:525-537`: "A selector is \"either the version number or the label used to retrieve the parameter value\" (`Parameter$Selector`) ... Err(_) => self.versions.iter().find(|held| held.labels.contains(selector)),"
- `src/ops/get_parameters_by_path.rs:394-396`: "A label points at ONE version of a parameter (\"A label can be assigned to only one version of a parameter at a time\", `LabelParameterVersion`)"
- `src/ops/label_parameter_version.rs:56-62` (modeled doc copied into the stub): "A version of a parameter can have a maximum of 10 labels. You can't attach the same label to different versions of the same parameter ... You can move a label from one version of a parameter to another."
- `src/ops/put_parameter.rs:693-702` already depends on labels existing: "label attached to it, Parameter Store won't delete the version and instead" ... `held.retained >= crate::state::MAX_VERSIONS_RETAINED && held.oldest_labelled`

## Steps to Reproduce

1. POST / with X-Amz-Target: AmazonSSM.PutParameter and {"Name":"/demo/db/password","Value":"v1","Type":"String"} — succeeds today, returns Version 1
2. POST AmazonSSM.PutParameter with {"Name":"/demo/db/password","Value":"v2","Type":"String","Overwrite":true} — returns Version 2
3. POST AmazonSSM.LabelParameterVersion with {"Name":"/demo/db/password","ParameterVersion":1,"Labels":["Production"]} — must answer 200 with ParameterVersion 1 and no InvalidLabels; today answers HTTP 501 NotImplemented
4. POST AmazonSSM.GetParameter with {"Name":"/demo/db/password:Production"} — must return the version-1 Parameter with Value "v1", Version 1 and Selector "Production"; today ParameterNotFound/no-such-label because nothing ever writes labels
5. POST AmazonSSM.LabelParameterVersion with {"Name":"/demo/db/password","Labels":["Production"]} (no ParameterVersion) — must MOVE the label to the latest version, so GetParameter("/demo/db/password:Production") then returns Value "v2", Version 2
6. POST AmazonSSM.GetParameterHistory with {"Name":"/demo/db/password"} — the version-2 entry must report Labels ["Production"] and the version-1 entry none
7. POST AmazonSSM.UnlabelParameterVersion with {"Name":"/demo/db/password","ParameterVersion":2,"Labels":["Production","NeverAttached"]} — must answer RemovedLabels ["Production"], InvalidLabels ["NeverAttached"]; today HTTP 501
8. POST AmazonSSM.GetParameter with {"Name":"/demo/db/password:Production"} — must now miss again (ParameterVersionNotFound/ParameterNotFound), proving the removal was persisted
9. POST AmazonSSM.DeleteParameters with {"Names":["/demo/db/password","/demo/absent"]} — must answer DeletedParameters ["/demo/db/password"], InvalidParameters ["/demo/absent"]; today HTTP 501
10. POST AmazonSSM.GetParameter with {"Name":"/demo/db/password"} — must answer 400 ParameterNotFound, proving the batch delete removed the stored record
