Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# The statusDetails refusal picks an arbitrary offender: the bound check walks the request's HashMap, so two identical requests can be refused with two different messages

_Filed: 2026-10-29T00:26:54Z_
_Operation: StartNextPendingJobExecution | Class: error-response_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`check_status_details` — the one bound check both `StartNextPendingJobExecution` and `UpdateJobExecution` refuse an over-bound `statusDetails` map with — iterates the request map (a `std::collections::HashMap`) and returns on the FIRST pair it finds over a bound. `HashMap` iteration order is not a function of the keys it holds, so when a request carries two or more offending pairs the name quoted in the `InvalidRequestException` message is whichever pair the iterator happened to reach first: two byte-identical requests can be answered with two different messages, and the same request can be answered differently between two runs of the same process.

This crate states the opposite rule for itself in two places and applies it everywhere else it has to turn a `HashMap` into an answer: `StartCommandExecution` sorts the `parameters` map by name before rendering it, for the stated reason that `HashMap` iteration order differs between two instances holding the same keys, and the stored details themselves are kept in a `BTreeMap` so "a response is deterministic". The refusal path is the one place where a `HashMap`'s order reaches the wire unnormalised.

The defect is in the message only — the status code (400 `InvalidRequestException`), the fact of the refusal, and the "nothing is written" guarantee are all correct and unaffected. Deciding the offender by a deterministic order (e.g. the smallest offending name) leaves every existing assertion untouched, because the existing test only asserts the code, never the message.

## Evidence

The non-deterministic walk — services/common/src/iot_job_executions.rs:104-121:
```
pub fn check_status_details(details: Option<&HashMap<String, String>>) -> Result<(), String> {
    let Some(details) = details else {
        return Ok(());
    };
    for (name, value) in details {
        let name_len = name.chars().count();
        if name_len > MAX_STATUS_DETAIL_NAME_LEN {
            return Err(format!(
                "a statusDetails name of {name_len} characters is over the {MAX_STATUS_DETAIL_NAME_LEN} character maximum"
            ));
        }
        let value_len = value.chars().count();
        if value_len > MAX_STATUS_DETAIL_VALUE_LEN {
            return Err(format!(
                "the statusDetails value for {name} is {value_len} characters, over the {MAX_STATUS_DETAIL_VALUE_LEN} character maximum"
            ));
        }
    }
```
Note `{name}` in the second message: the offender's own name is interpolated into the response body.

The handler that calls it directly — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_next_pending_job_execution.rs:199-212:
```
        // A `statusDetails` map is checked against the bounds a job execution's
        // details carry before anything is looked up: ...
        if let Err(reason) = crate::state::check_status_details(input.status_details.as_ref()) {
            return Err(
                StartNextPendingJobExecutionErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        message: Some(reason),
                    },
                )
                .into(),
            );
```
(`UpdateJobExecution` reaches the same function through `record.try_replace_status_details(...)` — src/ops/update_job_execution.rs:339-346 — and services/common/src/iot_job_executions.rs:434 `check_status_details(details)?;`.)

The sibling in THIS crate that normalises a `HashMap` for exactly this reason — src/ops/start_command_execution.rs:310-328:
```
/// The parameters as a JSON object, in SORTED name order — so one request always
/// yields one document and one fingerprint, whatever order the map arrived in.
/// `HashMap` iteration order differs between two instances holding the same keys,
/// and `serde_json::Map` preserves insertion order whenever anything in the
/// workspace enables `serde_json/preserve_order`, so the sort is what makes this
/// canonical rather than merely usually-equal.
fn parameters_json(
    parameters: Option<&std::collections::HashMap<String, CommandParameterValue>>,
) -> serde_json::Value {
    let mut pairs: Vec<(&String, &CommandParameterValue)> =
        parameters.into_iter().flatten().collect();
    pairs.sort_unstable_by_key(|(name, _)| *name);
```
And the same rule stated for the stored details — services/common/src/iot_job_executions.rs:249-250:
```
    /// Device-reported name/value pairs. Ordered, so a response is deterministic.
    pub status_details: BTreeMap<String, String>,
```

Test coverage: `status_details_are_bounded_and_an_over_bound_map_changes_nothing` (tests/all/job_execution_lifecycle.rs:1275) drives exactly ONE offending pair per row and asserts only `error_code(&refused) == "InvalidRequestException"` (lines 1349-1353, 1366-1370) — it never asserts the message, so making the offender deterministic changes no existing assertion.

## Steps to Reproduce

1. Start a command execution so thing `laser-1` holds a QUEUED execution (POST /command-executions with a valid `targetArn`/`commandArn`).
2. PUT /things/laser-1/jobs/$next with a body whose `statusDetails` carries TWO pairs that are both over the value bound, e.g. {"statusDetails": {"alpha": "<1025 v's>", "beta": "<1025 v's>"}}.
3. Observe 400 `InvalidRequestException`. The message names one of the two: `the statusDetails value for alpha is 1025 characters, ...` or `... for beta ...`.
4. Repeat the identical request in a fresh process (or send it to a build with a different `HashMap` seed) and observe the OTHER name reported for a byte-identical request.
5. The same happens for POST /things/laser-1/jobs/{jobId} (`UpdateJobExecution`), which reaches the same check through `JobExecutionRecord::try_replace_status_details`.
