// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ReportInstanceStatus`: what a submitted status report is
//! collected as, which instances may be reported on, and what the operation refuses.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness. Every assertion is
//! driven from a TABLE, because each rule under test holds over a CLASS of inputs — every
//! `ReportStatusType`, every `ReportInstanceReasonCodes` member, every instance state,
//! every id this service cannot resolve — and one recorded request is a single row of it.
//!
//! The rules under test are:
//!
//! 1. a report is COLLECTED, one per distinct instance the request names, carrying the
//!    status, the reason codes, the window and the description the request gave — and the
//!    call answers the envelope's success flag;
//! 2. the report changes nothing a reader reports: "Use of this action does not change the
//!    value returned by DescribeInstanceStatus", and the instance does not move;
//! 3. "The instance must be in the running state" — `pending` included, the state being
//!    the one the clock is about to make `running` — so an instance in any state on its
//!    way DOWN is refused `IncorrectInstanceState` and nothing is collected for it;
//! 4. an instance id this service cannot resolve is refused — malformed before unknown —
//!    and a request naming one collects nothing for any instance it names;
//! 5. a window whose end precedes its start, and a batch above the id ceiling, are
//!    refused `InvalidParameterValue` — the two rules the model cannot express, every
//!    constraint it DOES express being refused before this operation runs;
//! 6. `DryRun` answers whether the call WOULD have succeeded and collects nothing;
//! 7. a report the store has no room for — at either ceiling, its record count or its
//!    caller-chosen bytes — is refused rather than stored.
//!
//! What EC2's envelope puts AROUND the answer — the `<return>` flag, the request id and
//! the media type — is one rule for the whole service and is asserted over every
//! operation in [`super::ec2_query_envelope`]; rule 1 reads the flag back here only
//! because it is this operation's whole output.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::ops::types::{REPORT_INSTANCE_REASON_CODES_VALUES, REPORT_STATUS_TYPE_VALUES};
use ec2::state::{
    iso8601_utc, ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName,
    InstanceStatusReport, MAX_RESOURCE_IDS, MAX_STORED_BYTES, MAX_STORED_RECORDS_PER_STORE,
    TRANSITION_DELAY,
};

use super::ec2_query;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// `ReportStatusType`'s `ok` member, asserted against the model's own catalog in
/// [`statuses`] so a model change cannot leave this suite testing a word the service no
/// longer accepts.
const OK: &str = "ok";

/// `ReportStatusType`'s `impaired` member.
const IMPAIRED: &str = "impaired";

/// A `ReportInstanceReasonCodes` member the issue behind this suite did not cite, so the
/// rule is exercised on a reason code no recording named.
const PERFORMANCE_EBS_VOLUME: &str = "performance-ebs-volume";

/// A second such member, for the rows that submit more than one reason code.
const UNRESPONSIVE: &str = "unresponsive";

/// The start of the window a row may report over, in the form an ec2Query client sends a
/// `DateTime` in.
const WINDOW_START: &str = "2026-01-02T03:04:05Z";

/// The end of that window, an hour later.
const WINDOW_END: &str = "2026-01-02T04:04:05Z";

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The two `ReportStatusType` words this suite submits — the model's whole catalog, so a
/// row exists for every status the operation accepts.
fn statuses() -> [&'static str; 2] {
    assert_eq!(
        REPORT_STATUS_TYPE_VALUES,
        [IMPAIRED, OK],
        "the ReportStatusType catalog this suite tests against must be the model's"
    );
    [OK, IMPAIRED]
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One response, kept as text so each assertion can quote it.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    fn error_code(&self) -> String {
        ec2_query::error_code(&self.body)
    }

    fn error_message(&self) -> String {
        ec2_query::error_message(&self.body)
    }

    /// The envelope's `<return>` flag, as this operation's whole output.
    fn returned(&self) -> Option<String> {
        ec2_query::member(&self.body, "return")
    }
}

/// One EC2 emulator over its own engine. The engine is kept so the collected reports can
/// be read back out of the store: `ReportInstanceStatus` answers `Unit` and no operation
/// reports a submitted report back, because submitting one "does not change the value
/// returned by DescribeInstanceStatus" — so the store is the only place the collected
/// feedback can be observed.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: Vec<(String, String)>) -> Answer {
        let (status, body) = ec2_query::call(&self.router, REGION, action, &params).await;
        Answer { status, body }
    }

    /// Launch one instance and answer its id.
    async fn launch(&self, case: &str) -> String {
        let answer = self
            .call(
                "RunInstances",
                vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: RunInstances must succeed, got {} with body {}",
            answer.status, answer.body
        );
        ec2_query::element(&answer.body, "instanceId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: RunInstances must report the instance it minted: {}",
                answer.body
            )
        })
    }

    /// One instance in the `running` state, reached the way a caller reaches it: launch,
    /// then let the clock-owed `pending` -> `running` move fall due.
    async fn running_instance(&self, case: &str) -> String {
        let instance_id = self.launch(case).await;
        past_the_transition().await;
        assert_eq!(
            self.described_state(&instance_id).await,
            InstanceStateName::Running.as_str(),
            "case {case:?}: the launched instance must be running before it is reported on"
        );
        instance_id
    }

    /// Hold one instance still in `state`.
    ///
    /// The three TRANSIENT states (`pending`, `stopping`, `shutting-down`) cannot be held
    /// across a call through the operations that start them, since the clock moves the
    /// instance on after [`TRANSITION_DELAY`] whether or not anybody calls — so the state
    /// is written into the store the way `instance_attribute_reset` seeds the branches no
    /// operation can reach. The terminal states each row reaches through its own operation
    /// below.
    fn park(&self, case: &str, instance_id: &str, state: InstanceStateName) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard.instances.get_mut(instance_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold the launched instance {instance_id}")
        });
        instance.state = state;
        instance.pending_transition = None;
    }

    /// The `instanceState/name` `DescribeInstances` reports for one instance.
    async fn described_state(&self, instance_id: &str) -> String {
        ec2_query::described_instance_state(&self.router, REGION, instance_id).await
    }

    /// The reports collected about one instance, in submission order.
    fn collected(&self, instance_id: &str) -> Vec<InstanceStatusReport> {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard.instance_status_reports_for(instance_id).to_vec()
    }

    /// How many reports the store holds, across every instance.
    fn collected_everywhere(&self) -> usize {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard.instance_status_reports_held()
    }

    /// Charge the store's byte budget up to its ceiling, so the next write that carries
    /// any caller-chosen byte at all has no room — the branch rule 7 is about.
    fn fill_the_byte_budget(&self) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let room = MAX_STORED_BYTES.saturating_sub(guard.stored_bytes());
        guard.add_stored_bytes(room);
    }

    /// Fill the store's REPORT-COUNT ceiling, through the store's own writer, so the next
    /// submission has no record left to take — the other half of rule 7, and the ceiling
    /// the handler asks about first. Written straight into the store rather than through
    /// as many calls, which is what makes the row affordable.
    fn fill_the_report_ceiling(&self, instance_id: &str) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        while guard.has_instance_status_report_capacity(1) {
            guard.collect_instance_status_report(
                instance_id,
                InstanceStatusReport {
                    status: OK.to_string(),
                    reason_codes: vec![UNRESPONSIVE.to_string()],
                    start_time: None,
                    end_time: None,
                    description: None,
                    submitted_at: ::std::time::SystemTime::now(),
                },
            );
        }
    }
}

/// One row of the parameter table: the case, the form fields the request carries, and the
/// `<Code>` it is refused with (`None` for a request that is accepted).
type Row = (&'static str, Vec<(String, String)>, Option<&'static str>);

/// One submission: what the request says about the instances it names.
#[derive(Debug, Clone)]
struct Submission {
    status: &'static str,
    reason_codes: Vec<&'static str>,
    description: Option<&'static str>,
    window: Option<(&'static str, &'static str)>,
}

impl Submission {
    fn new(status: &'static str, reason_codes: Vec<&'static str>) -> Self {
        Self {
            status,
            reason_codes,
            description: None,
            window: None,
        }
    }

    fn described(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    fn over(mut self, start: &'static str, end: &'static str) -> Self {
        self.window = Some((start, end));
        self
    }

    /// The form fields for this submission against `instance_ids`, as ec2Query flattens
    /// each list into `InstanceId.N` / `ReasonCode.N` keys.
    fn fields(&self, instance_ids: &[String]) -> Vec<(String, String)> {
        let mut params = vec![p("Status", self.status)];
        for (index, instance_id) in instance_ids.iter().enumerate() {
            params.push(p(&format!("InstanceId.{}", index + 1), instance_id.clone()));
        }
        for (index, reason_code) in self.reason_codes.iter().enumerate() {
            params.push(p(&format!("ReasonCode.{}", index + 1), *reason_code));
        }
        if let Some(description) = self.description {
            params.push(p("Description", description));
        }
        if let Some((start, end)) = self.window {
            params.push(p("StartTime", start));
            params.push(p("EndTime", end));
        }
        params
    }
}

/// Assert that one collected report is the submission that produced it.
fn assert_is(case: &str, report: &InstanceStatusReport, submitted: &Submission) {
    assert_eq!(
        report.status, submitted.status,
        "case {case:?}: expected the submitted status {:?}, got {report:?}",
        submitted.status
    );
    assert_eq!(
        report.reason_codes,
        ec2_query::owned(&submitted.reason_codes),
        "case {case:?}: expected the submitted reason codes {:?}, got {report:?}",
        submitted.reason_codes
    );
    assert_eq!(
        report.description.as_deref(),
        submitted.description,
        "case {case:?}: expected the submitted description {:?}, got {report:?}",
        submitted.description
    );
    let window = submitted
        .window
        .map(|(start, end)| (start.to_string(), end.to_string()));
    let collected_window = match (report.start_time, report.end_time) {
        (Some(start), Some(end)) => Some((iso8601_utc(start), iso8601_utc(end))),
        _ => None,
    };
    assert_eq!(
        collected_window, window,
        "case {case:?}: expected the submitted window {window:?}, got {report:?}"
    );
}

/// Rule 1 and rule 2: a submission is collected once per distinct instance it names, as it
/// was submitted, and moves nothing.
///
/// The rows differ along what the rule generalises over: how many instances one call names
/// (and whether it names one of them twice), which status it reports, how many reason
/// codes it carries, and whether it carries a description and a window.
#[tokio::test]
async fn a_submission_is_collected_once_for_every_distinct_instance_it_names() {
    // (case, instances named, how many of them are distinct, the submission)
    let rows: Vec<(&str, usize, usize, Submission)> = vec![
        (
            "one instance, ok, one reason code",
            1,
            1,
            Submission::new(OK, vec![UNRESPONSIVE]),
        ),
        (
            "two instances, impaired, two reason codes, described, over a window",
            2,
            2,
            Submission::new(IMPAIRED, vec![PERFORMANCE_EBS_VOLUME, UNRESPONSIVE])
                .described("the attached volume stalls under load")
                .over(WINDOW_START, WINDOW_END),
        ),
        (
            "three instances, impaired, every reason code the model declares",
            3,
            3,
            Submission::new(IMPAIRED, REPORT_INSTANCE_REASON_CODES_VALUES.to_vec()),
        ),
        (
            "one instance named three times, ok",
            3,
            1,
            Submission::new(OK, vec![PERFORMANCE_EBS_VOLUME]),
        ),
        (
            "one instance, ok, a window that starts and ends at the same instant",
            1,
            1,
            Submission::new(OK, vec![UNRESPONSIVE]).over(WINDOW_START, WINDOW_START),
        ),
    ];

    for (case, named, distinct, submitted) in rows {
        let ec2 = Ec2::new();
        let mut instance_ids = Vec::new();
        for _ in 0..distinct {
            instance_ids.push(ec2.running_instance(case).await);
        }
        // A row that names fewer distinct instances than it names ids repeats the first
        // one, which is what the deduplication rule is about.
        let named_ids: Vec<String> = (0..named)
            .map(|index| instance_ids[index.min(distinct - 1)].clone())
            .collect();

        let answer = ec2
            .call("ReportInstanceStatus", submitted.fields(&named_ids))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.returned().as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return>, got {}",
            answer.body
        );
        assert!(
            ec2_query::member(&answer.body, "requestId")
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the envelope must state a request id, got {}",
            answer.body
        );

        for instance_id in &instance_ids {
            let collected = ec2.collected(instance_id);
            assert_eq!(
                collected.len(),
                1,
                "case {case:?}: expected exactly one collected report for {instance_id}, \
                 got {collected:?}"
            );
            assert_is(case, &collected[0], &submitted);
            // Rule 2: the instance did not move, and nothing was collected anywhere else.
            assert_eq!(
                ec2.described_state(instance_id).await,
                InstanceStateName::Running.as_str(),
                "case {case:?}: reporting on {instance_id} must not move it"
            );
        }
        assert_eq!(
            ec2.collected_everywhere(),
            distinct,
            "case {case:?}: expected {distinct} collected reports in all"
        );
    }
}

/// Rule 1, on repetition: a second submission about one instance is a second observation,
/// so both are kept, in submission order. The rows span both statuses the model declares.
#[tokio::test]
async fn every_submission_about_one_instance_is_collected() {
    let ec2 = Ec2::new();
    let case = "two submissions about one instance";
    let instance_id = ec2.running_instance(case).await;

    let mut submitted = Vec::new();
    for status in statuses() {
        let submission = Submission::new(status, vec![UNRESPONSIVE]);
        let answer = ec2
            .call(
                "ReportInstanceStatus",
                submission.fields(std::slice::from_ref(&instance_id)),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: reporting {status} must succeed, got {} with body {}",
            answer.status, answer.body
        );
        submitted.push(submission);
    }

    let collected = ec2.collected(&instance_id);
    assert_eq!(
        collected.len(),
        submitted.len(),
        "case {case:?}: expected one collected report per submission, got {collected:?}"
    );
    for (index, submission) in submitted.iter().enumerate() {
        assert_is(case, &collected[index], submission);
    }
}

/// Rule 3: "The instance must be in the running state", which an instance the clock is
/// about to make `running` is already on its way to. The table is the WHOLE
/// `InstanceStateName` catalog, so every state the service can hold an instance in is a
/// row, and a refused submission collects nothing.
#[tokio::test]
async fn only_a_running_instance_can_be_reported_on() {
    // (case, the state the instance is held in, the status the call expects)
    let rows: [(&str, InstanceStateName, u16); 6] = [
        ("a pending instance", InstanceStateName::Pending, 200),
        ("a running instance", InstanceStateName::Running, 200),
        (
            "an instance shutting down",
            InstanceStateName::ShuttingDown,
            400,
        ),
        ("a terminated instance", InstanceStateName::Terminated, 400),
        ("a stopping instance", InstanceStateName::Stopping, 400),
        ("a stopped instance", InstanceStateName::Stopped, 400),
    ];

    for (case, state, expected) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if state != InstanceStateName::Running {
            ec2.park(case, &instance_id, state);
        }

        let submitted = Submission::new(IMPAIRED, vec![UNRESPONSIVE]);
        let answer = ec2
            .call(
                "ReportInstanceStatus",
                submitted.fields(std::slice::from_ref(&instance_id)),
            )
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        if expected == 200 {
            assert_eq!(
                ec2.collected(&instance_id).len(),
                1,
                "case {case:?}: an instance at or on its way to running must collect the \
                 report"
            );
            continue;
        }
        assert_eq!(
            answer.error_code(),
            "IncorrectInstanceState",
            "case {case:?}: expected IncorrectInstanceState, got {}",
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            format!(
                "The instance '{instance_id}' is not in a state from which it can be \
                 reported on."
            ),
            "case {case:?}: the refusal must name the instance and the action its state \
             does not admit, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            ec2.collected_everywhere(),
            0,
            "case {case:?}: a refused submission must collect nothing"
        );
    }
}

/// Rule 4: an instance id this service cannot resolve is refused — malformed before
/// unknown — and the refusal is the answer however else the request is formed. The rows
/// span the forms a bad id takes: not an id at all, the null long id, a long id outside the
/// form EC2 issues, an id of another resource kind, and a well-formed id of either form
/// naming no instance.
#[tokio::test]
async fn an_instance_id_this_account_cannot_resolve_is_refused() {
    let ec2 = Ec2::new();
    // Launched so the refusal comes from the id in the request and not from an account
    // with nothing in it. It is also the SECOND id of each row below, so a refusal for one
    // instance of a batch collects nothing for the instance that was fine.
    let running = ec2.running_instance("seed").await;

    // (case, the id the request names, the expected <Code>)
    let rows: [(&str, &str, &str); 6] = [
        (
            "not an id at all",
            "not-an-instance",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a short-form id nothing was launched under",
            "i-12345678",
            "InvalidInstanceID.NotFound",
        ),
        (
            "a long id whose leading digit is above zero",
            "i-1234567890abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the null long id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a volume id",
            "vol-0123456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed id nothing was launched under",
            "i-0fedcba9876543210",
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (case, instance_id, expected) in rows {
        let submitted = Submission::new(OK, vec![UNRESPONSIVE]);
        let answer = ec2
            .call(
                "ReportInstanceStatus",
                submitted.fields(&[instance_id.to_string(), running.clone()]),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {}",
            answer.body
        );
        assert_eq!(
            ec2.collected_everywhere(),
            0,
            "case {case:?}: a refused submission must collect nothing, not even for the \
             instance it named that does resolve"
        );
    }
}

/// Rule 5: the values this operation itself refuses — the two the model cannot express.
///
/// A member the model DOES constrain (an absent `@required` one, a status or a reason code
/// outside its enum) never reaches this operation: the boundary validator refuses it in the
/// service-wide refusal envelope, which is its own issue and not this operation's wording
/// to assert. What is left to the handler is the window, whose two ends the model cannot
/// relate, and the batch size, for which the model publishes no length — so the rows are
/// the window the wrong way round and the id ceiling, at the maximum and one past it.
#[tokio::test]
async fn a_request_outside_the_operations_parameters_is_refused() {
    let ec2 = Ec2::new();
    let running = ec2.running_instance("seed").await;
    let many = |count: usize| -> Vec<(String, String)> {
        let mut params = vec![p("Status", OK), p("ReasonCode.1", UNRESPONSIVE)];
        for index in 0..count {
            // Well-formed ids of the form EC2 issues, so the ceiling is what refuses the
            // batch rather than an id inside it.
            params.push(p(
                &format!("InstanceId.{}", index + 1),
                format!("i-{index:017x}"),
            ));
        }
        params
    };

    // (case, the form fields, the expected <Code> — or None for a request that is accepted)
    let rows: Vec<Row> = vec![
        (
            "a window whose end precedes its start",
            vec![
                p("InstanceId.1", running.clone()),
                p("Status", OK),
                p("ReasonCode.1", UNRESPONSIVE),
                p("StartTime", WINDOW_END),
                p("EndTime", WINDOW_START),
            ],
            Some("InvalidParameterValue"),
        ),
        (
            "one id past the batch ceiling",
            many(MAX_RESOURCE_IDS + 1),
            Some("InvalidParameterValue"),
        ),
        (
            // At the ceiling the batch itself is accepted, so the refusal is about the
            // ids it names rather than about how many there are.
            "the batch ceiling exactly",
            many(MAX_RESOURCE_IDS),
            Some("InvalidInstanceID.Malformed"),
        ),
    ];

    for (case, params, expected) in rows {
        let answer = ec2.call("ReportInstanceStatus", params).await;
        match expected {
            Some(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case:?}: expected {code}, got {}",
                    answer.body
                );
            }
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                answer.status, answer.body
            ),
        }
        assert_eq!(
            ec2.collected_everywhere(),
            0,
            "case {case:?}: a refused submission must collect nothing"
        );
    }
}

/// Rule 6: `DryRun` asks whether the call WOULD have succeeded. A request that would have
/// been served is refused `DryRunOperation` and collects nothing; one that would have been
/// refused is refused with ITS reason instead. The rows span both.
#[tokio::test]
async fn a_dry_run_is_refused_and_collects_nothing() {
    // (case, whether the instance is left running, the expected status and code)
    let rows: [(&str, bool, u16, &str); 2] = [
        (
            "a dry run that would have been served",
            true,
            412,
            "DryRunOperation",
        ),
        (
            "a dry run that would have been refused",
            false,
            400,
            "IncorrectInstanceState",
        ),
    ];

    for (case, running, expected_status, expected_code) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if !running {
            ec2.park(case, &instance_id, InstanceStateName::Stopped);
        }

        let mut params =
            Submission::new(OK, vec![UNRESPONSIVE]).fields(std::slice::from_ref(&instance_id));
        params.push(p("DryRun", "true"));
        let answer = ec2.call("ReportInstanceStatus", params).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.body
        );
        assert_eq!(
            ec2.collected_everywhere(),
            0,
            "case {case:?}: a dry run must collect nothing"
        );
    }
}

/// Rule 7: the store's byte budget is asked BEFORE the first report is filed, so a
/// submission it has no room for is refused and NOTHING is collected — the description
/// carries caller-chosen bytes and the model publishes no length for it, so a count of
/// reports is no bound on what one holds.
#[tokio::test]
async fn a_report_the_store_has_no_room_for_is_refused() {
    let ec2 = Ec2::new();
    let case = "the byte budget is full";
    let instance_id = ec2.running_instance(case).await;
    ec2.fill_the_byte_budget();

    let submitted = Submission::new(IMPAIRED, vec![UNRESPONSIVE]).described("no room for this");
    let answer = ec2
        .call(
            "ReportInstanceStatus",
            submitted.fields(std::slice::from_ref(&instance_id)),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceLimitExceeded",
        "case {case:?}: expected ResourceLimitExceeded, got {}",
        answer.body
    );
    assert_eq!(
        ec2.collected(&instance_id).len(),
        0,
        "case {case:?}: a submission the store has no room for must collect nothing"
    );
}

/// Rule 7, the other ceiling: the number of reports the store holds is asked about BEFORE
/// the first report is filed, so a submission past it is refused and collects nothing —
/// a count of records being the bound the byte budget cannot provide for a report that
/// carries no description at all.
#[tokio::test]
async fn a_report_past_the_store_ceiling_is_refused() {
    let ec2 = Ec2::new();
    let case = "the report ceiling is full";
    let instance_id = ec2.running_instance(case).await;
    ec2.fill_the_report_ceiling(&instance_id);
    let held = ec2.collected_everywhere();
    assert_eq!(
        held, MAX_STORED_RECORDS_PER_STORE,
        "case {case:?}: the store must be at its report ceiling before the call, got {held}"
    );

    let submitted = Submission::new(OK, vec![UNRESPONSIVE]);
    let answer = ec2
        .call(
            "ReportInstanceStatus",
            submitted.fields(std::slice::from_ref(&instance_id)),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceLimitExceeded",
        "case {case:?}: expected ResourceLimitExceeded, got {}",
        answer.body
    );
    assert_eq!(
        ec2.collected_everywhere(),
        held,
        "case {case:?}: a submission past the ceiling must collect nothing"
    );
}
