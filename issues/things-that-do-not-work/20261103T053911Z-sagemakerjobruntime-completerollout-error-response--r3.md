Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3124d004fa442bade15aa914c0814f4c097c662f

# A refusal that includes a malformed JobArn silently drops every other collected constraint violation

_Filed: 2026-11-03T05:39:11Z_
_Operation: CompleteRollout | Class: error-response_

> _Note: this issue was identified against the code base at commit `3124d004fa442bade15aa914c0814f4c097c662f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service::refusal_message` is the ONE renderer both refusal paths of this crate reach (the pre-dispatch `InModelledEnvelope` walk and the clause `crate::ops::complete_rollout::enum_violation` words itself). When the collected violations contain a `jobArn` `@pattern` violation whose value is known, the function RETURNS EARLY with the single malformed-ARN sentence and discards every other violation in the slice — the `trajectoryId` `@length` clause, the `Status` enum-value clause, the `jobArn` `@length` clause. A `CompleteRollout` request that breaks three modelled constraints at once is therefore answered "Input arn <value> is malformed" and told nothing about the other two members, and the "N validation errors detected" count the envelope would state is lost.

This contradicts the function's own documentation, which says only the WORDING of that one class differs (it is reported "as a malformed ARN naming the value it was given, rather than as a regular-expression clause" — a clause-level substitution), and the sibling helper's documentation, which states the rule that "the clauses of one refusal are joined into ONE `Message`". It is also asymmetric inside the same function: when the ARN clause captured NO value the early return is skipped and the other members' clauses DO survive into `constraints::envelope(violations)`, so whether a caller is told about its bad `TrajectoryId` depends on whether the ARN clause carried a value.

The crate's own unit test names exactly the violation set that triggers this ("four, the most its models' constrained members could ever break at once (`JobArn` length and pattern, `TrajectoryId` length, `Status` value set)") and asserts the multi-clause envelope "N validation errors detected: " — but every row it builds uses the path `trajectoryId`, so `is_malformed_arn` never matches and the four-clause case the comment names is never actually rendered through the branch that collapses it.

## Evidence

The code that drops the other clauses — services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service.rs:188-208:
```rust
pub(crate) fn refusal_message(violations: &[Violation]) -> Option<String> {
    // Only when the value is actually known: a pattern violation that captured none
    // would otherwise be reported as "Input arn  is malformed", which tells the caller
    // nothing, so the framework's own clause is kept for it instead.
    let malformed_arn = violations
        .iter()
        .filter(|violation| is_malformed_arn(violation))
        .find_map(|violation| violation.value.as_deref());
    if let Some(value) = malformed_arn {
        // One clause, one value: its room is the whole member less this wording's prose.
        let room = LARGEST_MESSAGE_BYTES.saturating_sub(malformed_arn_prose_bytes());
        return Some(malformed_arn_message(
            &service_common::validation::echo_held_to(value, room),
        ));
    }
    let rendered = constraints::envelope(violations)?;
```
(`violations` is never consulted again on that path.)

The rule the same function's doc states — src/service.rs:172-180:
"Every clause comes from the framework's own constraint renderer (`constraints::envelope`), so a constraint class this service has never seen cannot arrive with a sentence of its own. ONE class is worded differently: a value that is not an ARN at all. … the service reports a value that fails it as a malformed ARN naming the value it was given, **rather than as a regular-expression clause**".

The sibling helper's doc — src/service.rs:213-215:
"Why a share rather than a per-clause ceiling: the clauses of one refusal are joined into ONE `Message`, so what they may quote is bounded TOGETHER".

The crate's own test names the triggering set but never renders it — src/service.rs:634-640 and 672:
"// SEVERAL clauses, every one of them quoting a value: rows differ along the clause count — two, the three a request to this service can reach, and four, the most its models' constrained members could ever break at once (`JobArn` length and pattern, `TrajectoryId` length, `Status` value set; …)" followed by `assert!(message.starts_with(&format!("{clauses} validation errors detected: ")), …)` — while every row is built over `path: LONGEST_MEMBER_PATH.to_string()` (src/service.rs:561, 567), i.e. `trajectoryId`, so `is_malformed_arn` (src/service.rs:164-167: `violation.path.rsplit('.').next() == Some(ARN_MEMBER_PATH) && matches!(violation.kind, ViolationKind::Pattern(_))`) never matches in the test.

Test coverage of the surviving behaviour: tests/all/modelled_rejections.rs:357-437 (`a_job_arn_that_is_not_an_arn_is_reported_as_malformed`) only ever sends a bad ARN together with a VALID `TrajectoryId`/`Status`, so it pins the single-clause sentence `format!("Input arn {arn} is malformed")` and would remain green if the other clauses were kept; no suite in the crate drives a request that breaks the ARN pattern and another modelled constraint at the same time.

## Steps to Reproduce

1. Mount the service as tests/all/harness.rs `mounted_service()` does.
2. POST /complete-rollout with header `X-Amzn-SageMaker-Job-Arn: not-a-valid-job-arn` and body `{"TrajectoryId": "<257 't' characters>", "Status": "bogus-status", "ClientToken": "tok"}` — three modelled constraints are broken at once (`JobArn` @pattern, `TrajectoryId` @length max 256, `Status` outside the `CompletionStatus` value set).
3. Read the `Message` member of the 400 ValidationException document.
4. Observed: `Input arn not-a-valid-job-arn is malformed` only — the `trajectoryId` length clause and the `status` enum clause collected by the same walk are discarded, and the sentence states no error count.
5. Contrast: send the same request with an EMPTY `X-Amzn-SageMaker-Job-Arn` header value instead (so the ARN clause's value is empty) and the other members' clauses survive into `constraints::envelope`, showing the drop is conditional on the ARN clause carrying a value.
