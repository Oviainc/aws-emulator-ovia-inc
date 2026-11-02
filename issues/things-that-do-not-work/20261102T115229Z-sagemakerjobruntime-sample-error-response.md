Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: fdc7c4a374233335cd2732abbc09ac690a8a7f08

# An empty JobArn header is refused with the value-less "Input arn  is malformed" sentence the code's own comment says must never be answered

_Filed: 2026-11-02T11:52:29Z_
_Operation: Sample | Class: error-response_

> _Note: this issue was identified against the code base at commit `fdc7c4a374233335cd2732abbc09ac690a8a7f08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service::refusal_message` words a `JobArn` that misses the modelled ARN `@pattern` as this service's own malformed-ARN sentence, and it guards that branch so the sentence is only used when the refused value is KNOWN — the comment states, in those words, that a pattern violation carrying no value "would otherwise be reported as 'Input arn  is malformed', which tells the caller nothing, so the framework's own clause is kept for it instead".

The guard only excludes an ABSENT member, not a PRESENT-but-empty one. `Violation::value` is documented by the shared collector as carrying exactly that distinction: `None` means the member was absent, so a caller that sends an EMPTY `X-Amzn-SageMaker-Job-Arn` header reaches `refusal_message` with `value: Some("")`, `find_map(|violation| violation.value.as_deref())` yields `Some("")`, and the branch answers the literal sentence the comment names as useless: `Input arn  is malformed` (two spaces, no value, no requirement).

Two things are lost with it. First, the caller is told nothing it can act on — which is the stated reason the guard exists at all. Second, the `@length` minimum clause the framework collected for the SAME member (`Value '' at 'jobArn' failed to satisfy constraint: Member must have length greater than or equal to 1`) is discarded by the early return, so the one clause that does state the broken requirement never reaches the wire.

The case is reachable on every operation of this service (all four bind `JobArn`); `Sample` is named because the crate's own suite already drives exactly this request on `/sample` and `/sample-with-response-stream` with an assertion loose enough to pass on the useless sentence.

## Evidence

The rule the code states for itself, `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service.rs:188-202`:

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

the sentence it produces, `src/service.rs:150-152`:

    fn malformed_arn_message(value: &str) -> String {
        format!("Input arn {value} is malformed")
    }

why `Some("")` is not "captured none" — the shared collector's own definition, `services/common/src/validation.rs:253-256`:

    /// The distinction [`Violation::value`] cannot carry: `None` there means the member was
    /// ABSENT, so a value that was PRESENT but has no text rendering (a blob's bytes, a map, a
    /// document) reaches a service as the framework's summary of it

that an empty string DOES produce a pattern violation carrying `Some("")`, `vendor/smithy-schema-server/src/constraints.rs:346-354`:

    fn check(&self, value: &str, path: &str) -> Option<Violation> {
        if pattern_matches(self.regex, value) {
            return None;
        }
        Some(Violation {
            path: path.to_string(),
            kind: ViolationKind::Pattern(self.regex.to_string()),
            value: Some(echoed(value).into_owned()),
        })

and that the length clause is collected ALONGSIDE it rather than instead of it, `vendor/smithy-schema-server/src/constraints.rs:835-859`:

        if let Some(len) = len {
            if let Some(v) = check_length(traits, len, value, path) {
                self.push_violation(v);
                ...
        if len.is_some() && numeric.is_none() {
            if let Some(v) = check_pattern(traits, value, path) {
                self.push_violation(v);

which is also what this crate states of its own model, `src/service.rs:632-636` (test doc):

        // SEVERAL clauses, every one of them quoting a value: rows differ along the clause
        // count — two, the three a request to this service can reach, and four, the most its
        // models' constrained members could ever break at once (`JobArn` length and pattern,
        // `TrajectoryId` length, `Status` value set; ...)

Test coverage: the empty `JobArn` row is driven but only loosely asserted, `tests/all/job_arn_is_the_callers_job.rs:111` and `:148-154`:

        ("the empty value, below the 1 minimum", String::new()),
    ...
            assert!(
                !reply.message().is_empty()
                    && reply.message() != state::job_not_found_message(&arn),

so `Input arn  is malformed` passes today. That an empty HEADER-bound member arrives present-and-empty (rather than absent) is pinned for the sibling header member, `tests/all/refusal_fits_the_modelled_message_member.rs:356-368`:

        (
            "an empty trajectory id, header-bound",
            SAMPLE_PATH,
            Some(""),
            "Member must have length greater than or equal to 1",
        ),

and `tests/all/rejected_value_echo.rs:269-280` pins the same empty-value clause (`Value '' at 'trajectoryId' ...`), so nothing in the crate pins the `jobArn` wording for the empty value either way.

## Steps to Reproduce

1. Mount the service as tests/all/harness.rs::mounted_service does and register a job in the shared directory (arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/run-1).
2. POST /sample with headers X-Amzn-SageMaker-Job-Arn: "" (present, empty), X-Amzn-SageMaker-Trajectory-Id: traj-1, content-type: application/json, and an OpenAI-shaped JSON body.
3. Observe 400 ValidationException whose Message is `Input arn  is malformed` — the exact sentence src/service.rs:189-191 says must not be answered, naming no value and no requirement.
4. Note the clause the framework collected for the same member is gone: `1 validation error detected: Value '' at 'jobArn' failed to satisfy constraint: Member must have length greater than or equal to 1` is never rendered, because the malformed-ARN branch returns before `constraints::envelope(violations)` is reached.
5. Compare with the sibling header member: the same empty value on X-Amzn-SageMaker-Trajectory-Id is refused with the length clause quoting '' (tests/all/refusal_fits_the_modelled_message_member.rs:356-362), showing an empty header reaches the walk as a present value with Some("").
