// SPDX-License-Identifier: CC0-1.0

//! How LONG a refusal this service words may be, end to end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness [`crate::virtual_cluster_job_run_loop`] owns) and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule under test: a modelled error's `message` never runs past the maximum the
//! member's own shape declares for it — every error of this service carries its `message`
//! as a `String1024`, len 1..1024 (smithy_query: shape `ResourceNotFoundException`) —
//! however long the request value the refusal reports is, and the wording AROUND that
//! value stays whole, so the value is what gives up the characters there is no room for.
//!
//! The tables span what the rule generalises over: which request value the refusal
//! reports (an `@httpLabel` identifier a `String2048` admits 2048 characters of, a job
//! template parameter value a `String1024` admits 1024 of, a list of up to a hundred
//! 512-character parameter names), which operation issues it, which modelled error
//! carries it, which of the three refusals a job template application words carries it (a
//! value that is not of the declared type, the parameters the template declares nothing
//! about, the references nothing resolves), and the LENGTH of that value — the lengths on
//! both sides of the longest one the message still holds whole, the model's own maximum,
//! and a multi-byte value, because the bound counts characters rather than bytes.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state::{
    self, MESSAGE_MAX_CHARACTERS,
};

use crate::resource_tag_removal::encoded;
use crate::virtual_cluster_job_run_loop::{text, Emr};

/// The errors the operations below carry their refusal in (smithy_query: operations
/// `GetManagedEndpointSessionCredentials`, `StartJobRun`).
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_REQUEST: &str = "ValidationException";

/// The fixed tail of the one not-found wording of this service, which the suite
/// [`crate::not_found_message`] pins the whole of.
const NOT_FOUND_TAIL: &str = " doesn't exist.";

/// How this service spells the two resource types the calls below report missing.
const VIRTUAL_CLUSTER: &str = "Virtual cluster";
const ENDPOINT: &str = "Endpoint";

/// The characters a message has left for the value it reports, beside `wording` characters
/// of fixed text: the rule's arithmetic, written here from the modelled bound.
fn room_for_the_value(wording: usize) -> usize {
    MESSAGE_MAX_CHARACTERS - wording
}

/// Hold one refused call's `message` to the rule: inside the modelled bound, the wording
/// whole, and the reported value exactly as much of the request's as there was room for —
/// no less, so a refusal is not clipped further than the bound asks.
fn assert_bounded(
    case: &str,
    message: &str,
    prefix: &str,
    suffix: &str,
    sent_value: &str,
) {
    let characters = message.chars().count();
    assert!(
        characters <= MESSAGE_MAX_CHARACTERS,
        "case {case:?}: the message must hold at most {MESSAGE_MAX_CHARACTERS} characters, \
         got {characters} ({message:?})"
    );
    assert!(
        message.starts_with(prefix) && message.ends_with(suffix),
        "case {case:?}: expected the wording {prefix:?} … {suffix:?} to stay whole, got \
         {message:?}"
    );
    let reported = &message[prefix.len()..message.len() - suffix.len()];
    let room = room_for_the_value(prefix.chars().count() + suffix.chars().count());
    let expected: String = sent_value.chars().take(room).collect();
    assert_eq!(
        reported,
        expected,
        "case {case:?}: expected the refusal to report the first {} characters of the value \
         it is about, got {} of them ({reported:?})",
        expected.chars().count(),
        reported.chars().count()
    );
}

/// One acceptable `GetManagedEndpointSessionCredentials` body: the two members the model
/// marks `@required` beside the two `@httpLabel` identifiers (smithy_query: operation
/// `GetManagedEndpointSessionCredentials`).
fn session_body() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
        "credentialType": state::SESSION_CREDENTIAL_TYPE_TOKEN,
    })
}

/// Identifier lengths spanning the class the rule generalises over, for a refusal whose
/// fixed wording is `wording` characters long: the shortest a `String2048` admits, a
/// `ResourceIdString`'s own maximum (len 1..64 — smithy_query: shape `ResourceIdString`),
/// the three lengths around the longest identifier the message still holds whole, the
/// 2048 characters `String2048` admits (smithy_query: shape `String2048`), and a
/// multi-byte identifier.
fn identifier_lengths(wording: usize) -> Vec<(String, String)> {
    let room = room_for_the_value(wording);
    vec![
        (
            "the shortest identifier the model admits".to_string(),
            "a".to_string(),
        ),
        (
            "a ResourceIdString's own maximum".to_string(),
            "z".repeat(64),
        ),
        (
            format!("{} characters, one inside the longest the message holds", room - 1),
            "b".repeat(room - 1),
        ),
        (
            format!("{room} characters, exactly the longest the message holds"),
            "c".repeat(room),
        ),
        (
            format!("{} characters, one past it", room + 1),
            "d".repeat(room + 1),
        ),
        ("1500 characters".to_string(), "e".repeat(1500)),
        (
            "the longest identifier the model admits".to_string(),
            "f".repeat(2048),
        ),
        (
            "1024 multi-byte characters".to_string(),
            // 1024 characters at two bytes each: the longest multi-byte identifier that
            // is inside `String2048` however the boundary counts a length.
            "é".repeat(1024),
        ),
    ]
}

/// THE RULE for the `String2048` identifiers of `GetManagedEndpointSessionCredentials` —
/// the one operation of this service whose resource identifiers may be longer than the
/// message that reports them missing: the not-found refusal stays inside the modelled
/// bound and still reads as the not-found refusal it is.
#[tokio::test]
async fn a_not_found_refusal_stays_inside_the_modelled_message_bound() {
    let emr = Emr::new();
    // A cluster that really exists, so a call aimed at an absent ENDPOINT is refused for
    // the endpoint rather than for its cluster.
    let host = emr.cluster("bound-host", "tok-bound-host").await;

    let cluster_prefix = format!("{VIRTUAL_CLUSTER} ");
    let endpoint_prefix = format!("{ENDPOINT} ");
    let wordings = [
        (VIRTUAL_CLUSTER, cluster_prefix.clone()),
        (ENDPOINT, endpoint_prefix.clone()),
    ];

    for (resource_type, prefix) in &wordings {
        let wording = prefix.chars().count() + NOT_FOUND_TAIL.chars().count();
        for (label, identifier) in identifier_lengths(wording) {
            let case = format!("{resource_type} named by {label}");
            let path = if *resource_type == VIRTUAL_CLUSTER {
                format!(
                    "/virtualclusters/{}/endpoints/someendpoint/credentials",
                    encoded(&identifier)
                )
            } else {
                format!(
                    "/virtualclusters/{host}/endpoints/{}/credentials",
                    encoded(&identifier)
                )
            };
            let answer = emr.send("POST", &path, Some(session_body())).await;
            answer.refused(&case, 400, NOT_FOUND);
            let message = text(&answer.body, "/message", &case);
            assert_bounded(&case, &message, prefix, NOT_FOUND_TAIL, &identifier);
        }
    }
}

/// THE SAME RULE at another operation, another modelled error and another class of
/// reported value: `StartJobRun`'s refusals of a job template application report values
/// the request chose — one parameter value of up to the 1024 characters a
/// `TemplateParameterInputMap` value admits, and up to a hundred 512-character parameter
/// names at once (smithy_query: shape `TemplateParameterInputMap`) — and are held to the
/// same bound.
#[tokio::test]
async fn a_job_template_refusal_reporting_a_long_value_stays_inside_the_bound() {
    let emr = Emr::new();
    let cluster = emr.cluster("bound-template-host", "tok-bound-template").await;
    let role = "arn:aws:iam::000000000000:role/EMRJobRole";
    let driver = json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}});

    // A template that declares one NUMBER parameter, so a value that is not a number is
    // refused with the value reported.
    let numeric_template = text(
        emr.send(
            "POST",
            "/jobtemplates",
            Some(json!({
                "name": "bound-numeric",
                "clientToken": "tok-bound-numeric",
                "jobTemplateData": {
                    "executionRoleArn": role,
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": driver,
                    "parameterConfiguration": {"scale": {"type": "NUMBER"}},
                },
            })),
        )
        .await
        .ok("a template declaring a NUMBER parameter"),
        "/id",
        "a template declaring a NUMBER parameter",
    );
    // A template that declares NO parameter, so every parameter a request supplies for it
    // is undeclared and the whole list of them is reported.
    let plain_template = text(
        emr.send(
            "POST",
            "/jobtemplates",
            Some(json!({
                "name": "bound-plain",
                "clientToken": "tok-bound-plain",
                "jobTemplateData": {
                    "executionRoleArn": role,
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": driver,
                },
            })),
        )
        .await
        .ok("a template declaring no parameter"),
        "/id",
        "a template declaring no parameter",
    );

    // The longest value a `TemplateParameterInputMap` admits, which is the message bound
    // itself, so no wording at all fits beside the whole of it.
    let long_value = "9x".repeat(512);
    let case = "a NUMBER parameter given 1024 characters that are not a number";
    let answer = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "clientToken": "tok-bound-number",
                "jobTemplateId": numeric_template,
                "jobTemplateParameters": {"scale": long_value.clone()},
            })),
        )
        .await;
    answer.refused(case, 400, INVALID_REQUEST);
    let message = text(&answer.body, "/message", case);
    assert_bounded(
        case,
        &message,
        "Job template parameter scale is declared as NUMBER, so ",
        " is not a valid value for it.",
        &long_value,
    );

    // A hundred parameters at the 512 characters `TemplateParameterName` admits, none of
    // them declared: the list the refusal reports is far longer than the message.
    let mut names: Vec<String> = (0..100)
        .map(|index| format!("p{index:0>3}{}", "x".repeat(508)))
        .collect();
    names.sort();
    let supplied: Value = Value::Object(
        names
            .iter()
            .map(|name| (name.clone(), json!("1")))
            .collect(),
    );
    let case = "a hundred undeclared parameters of 512 characters each";
    let answer = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "clientToken": "tok-bound-undeclared",
                "jobTemplateId": plain_template,
                "jobTemplateParameters": supplied,
            })),
        )
        .await;
    answer.refused(case, 400, INVALID_REQUEST);
    let message = text(&answer.body, "/message", case);
    assert_bounded(
        case,
        &message,
        &format!("Job template {plain_template} declares no parameter named ["),
        "].",
        &names.join(", "),
    );

    // The control: the refusals above started nothing, so the bound is about the wording
    // of a refusal and nothing else.
    let runs = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    assert_eq!(
        runs.ok("the cluster's job runs")
            .pointer("/jobRuns")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused StartJobRun must have started nothing, got {}",
        runs.body
    );
}

/// An `IAMRoleArn` the run's own member accepts (smithy_query: shape `IAMRoleArn`), so a
/// template below is refused for what it says about its PARAMETERS and nothing else.
const ROLE: &str = "arn:aws:iam::000000000000:role/EMRJobRole";

/// A `ReleaseLabel` the run's own member accepts (smithy_query: shape `ReleaseLabel`).
const RELEASE: &str = "emr-6.15.0-latest";

/// The parameter the value table's template declares `NUMBER`.
const NUMERIC_PARAMETER: &str = "instances";

/// How a refusal renders a LIST of parameter names: what stands between two of them.
const SEPARATOR: &str = ", ";

/// The longest ONE parameter name the model admits, and so the longest name a row below
/// supplies (smithy_query: shape `TemplateParameterName`).
const PARAMETER_NAME_MAX: usize = 512;

/// `CreateJobTemplate` for `data`, answering with the id the service minted for it.
async fn stored_template(emr: &Emr, name: &str, token: &str, data: Value) -> String {
    let answer = emr
        .send(
            "POST",
            "/jobtemplates",
            Some(json!({"name": name, "clientToken": token, "jobTemplateData": data})),
        )
        .await;
    text(answer.ok(name), "/id", name)
}

/// The three members `JobTemplateData` marks `@required` (smithy_query: shape
/// `JobTemplateData`), with `spark_submit_parameters` carrying whatever the row wants
/// substituted: a template below is refused for what it says about its parameters, so
/// everything else about it is acceptable.
fn template_data(spark_submit_parameters: Option<String>) -> Value {
    let mut driver = json!({"entryPoint": "s3://bucket/j.py"});
    if let Some(parameters) = spark_submit_parameters {
        driver["sparkSubmitParameters"] = json!(parameters);
    }
    json!({
        "executionRoleArn": ROLE,
        "releaseLabel": RELEASE,
        "jobDriver": {"sparkSubmitJobDriver": driver},
    })
}

/// `StartJobRun` naming `template`, with the parameters the row supplies for it.
async fn templated_run(
    emr: &Emr,
    cluster: &str,
    token: &str,
    template: &str,
    parameters: Option<Value>,
) -> crate::virtual_cluster_job_run_loop::Answer {
    let mut body = json!({"clientToken": token, "jobTemplateId": template});
    if let Some(parameters) = parameters {
        body["jobTemplateParameters"] = parameters;
    }
    emr.send(
        "POST",
        &format!("/virtualclusters/{cluster}/jobruns"),
        Some(body),
    )
    .await
}

/// The control every table below closes with: a refusal is wording, so not one of the
/// refused submissions started a run.
async fn assert_nothing_started(emr: &Emr, cluster: &str, case: &str) {
    let runs = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    let started = runs
        .ok(case)
        .pointer("/jobRuns")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(
        started,
        Some(0),
        "case {case:?}: expected every refused submission to have started nothing, got \
         {started:?} runs ({})",
        runs.body
    );
}

/// THE RULE at the refusal that reports ONE caller-chosen value: a parameter the template
/// declared `NUMBER` and the request gave a value that is not a number.
///
/// The rows span the length class the rule generalises over: the shortest value the model
/// admits, the three lengths around the longest value this message still holds whole, the
/// 1024 characters a `TemplateParameterInputMap` value admits (smithy_query: shape
/// `TemplateParameterInputMap`), and a multi-byte value, because the bound counts
/// characters rather than bytes.
#[tokio::test]
async fn a_job_template_value_refusal_is_bounded_at_every_length() {
    let emr = Emr::new();
    let cluster = emr
        .cluster("bound-value-host", "tok-bound-value-host")
        .await;
    let mut data = template_data(Some(format!(
        "--conf spark.executor.instances=${{{NUMERIC_PARAMETER}}}"
    )));
    data["parameterConfiguration"] = json!({NUMERIC_PARAMETER: {"type": "NUMBER"}});
    let template = stored_template(&emr, "bound-value", "tok-bound-value", data).await;

    let prefix =
        format!("Job template parameter {NUMERIC_PARAMETER} is declared as NUMBER, so ");
    let suffix = " is not a valid value for it.";
    let room = room_for_the_value(prefix.chars().count() + suffix.chars().count());
    // None of these parses as a number, which is what makes the submission a refusal.
    let rows: Vec<(String, String)> = vec![
        (
            "the shortest value the model admits".to_string(),
            "x".to_string(),
        ),
        (
            format!("{} characters, one inside the longest the message holds", room - 1),
            "y".repeat(room - 1),
        ),
        (
            format!("{room} characters, exactly the longest the message holds"),
            "z".repeat(room),
        ),
        (
            format!("{} characters, one past it", room + 1),
            "w".repeat(room + 1),
        ),
        (
            "the 1024 characters the model admits".to_string(),
            "v".repeat(1024),
        ),
        (
            // 512 characters at two bytes each, inside `String1024` however the boundary
            // counts a length.
            "512 multi-byte characters".to_string(),
            "é".repeat(512),
        ),
    ];

    for (index, (label, value)) in rows.iter().enumerate() {
        let case = format!("a NUMBER parameter given {label}");
        let supplied = Value::Object(
            [(NUMERIC_PARAMETER.to_string(), json!(value))]
                .into_iter()
                .collect(),
        );
        let answer = templated_run(
            &emr,
            &cluster,
            &format!("tok-bound-value-{index}"),
            &template,
            Some(supplied),
        )
        .await;
        answer.refused(&case, 400, INVALID_REQUEST);
        let message = text(&answer.body, "/message", &case);
        assert_bounded(&case, &message, &prefix, suffix, value);
    }

    assert_nothing_started(&emr, &cluster, "the value table's cluster").await;
}

/// One row of the two tables whose refusal reports a LIST of parameter names.
struct NameList {
    label: String,
    names: Vec<String>,
}

/// `count` distinct parameter names of `length` characters each, in the order a refusal
/// reports them: both refusals below collect the names into a set and join them, so that
/// order is byte order. Every name is inside the 512 characters `TemplateParameterName`
/// admits (smithy_query: shape `TemplateParameterName`) and is also a reference name the
/// template grammar admits, so one list serves both tables.
fn parameter_names(count: usize, length: usize) -> Vec<String> {
    let mut names: Vec<String> = (0..count)
        .map(|index| format!("p{index:0>4}{}", "x".repeat(length - 5)))
        .collect();
    names.sort();
    names
}

/// Two parameter names whose joined rendering is EXACTLY `characters` long, so a row can
/// sit on the bound itself rather than near it.
fn names_joining_to(characters: usize) -> Vec<String> {
    let shorter = characters - PARAMETER_NAME_MAX - SEPARATOR.chars().count();
    assert!(
        shorter <= PARAMETER_NAME_MAX,
        "a list joining to {characters} characters needs a name of {shorter}, past the \
         {PARAMETER_NAME_MAX} characters the model admits"
    );
    vec!["a".repeat(shorter), "b".repeat(PARAMETER_NAME_MAX)]
}

/// The rows both name-list tables run, spanning what that half of the rule generalises
/// over: how many names the refusal reports and how long each one is, with the two lists
/// around the longest rendering the message still holds whole.
fn name_list_rows(room: usize) -> Vec<NameList> {
    vec![
        NameList {
            label: "one name of 8 characters".to_string(),
            names: parameter_names(1, 8),
        },
        NameList {
            label: format!("one name of the {PARAMETER_NAME_MAX} characters the model admits"),
            names: parameter_names(1, PARAMETER_NAME_MAX),
        },
        NameList {
            label: format!("two names joining to exactly the {room} the message holds"),
            names: names_joining_to(room),
        },
        NameList {
            label: format!("two names joining to {}, one past it", room + 1),
            names: names_joining_to(room + 1),
        },
        NameList {
            label: format!("three names of {PARAMETER_NAME_MAX} characters each"),
            names: parameter_names(3, PARAMETER_NAME_MAX),
        },
        NameList {
            label: format!("fifty names of {PARAMETER_NAME_MAX} characters each"),
            names: parameter_names(50, PARAMETER_NAME_MAX),
        },
    ]
}

/// THE SAME RULE at the refusal that reports the parameters a request supplied and the
/// template declares nothing about: the whole list is the caller's, so it is what gives up
/// the characters the message has no room for.
#[tokio::test]
async fn an_undeclared_parameter_refusal_is_bounded_however_many_names_it_reports() {
    let emr = Emr::new();
    let cluster = emr
        .cluster("bound-undeclared-host", "tok-bound-undeclared-host")
        .await;
    let template = stored_template(
        &emr,
        "bound-undeclared",
        "tok-bound-undeclared",
        template_data(None),
    )
    .await;

    let prefix = format!("Job template {template} declares no parameter named [");
    let suffix = "].";
    let room = room_for_the_value(prefix.chars().count() + suffix.chars().count());

    for (index, row) in name_list_rows(room).into_iter().enumerate() {
        let case = format!("undeclared parameters: {}", row.label);
        let supplied = Value::Object(
            row.names
                .iter()
                .map(|name| (name.clone(), json!("1")))
                .collect(),
        );
        let answer = templated_run(
            &emr,
            &cluster,
            &format!("tok-bound-undeclared-{index}"),
            &template,
            Some(supplied),
        )
        .await;
        answer.refused(&case, 400, INVALID_REQUEST);
        let message = text(&answer.body, "/message", &case);
        assert_bounded(
            &case,
            &message,
            &prefix,
            suffix,
            &row.names.join(SEPARATOR),
        );
    }

    assert_nothing_started(&emr, &cluster, "the undeclared-parameter table's cluster").await;
}

/// THE SAME RULE at the third refusal a template application words — the references the
/// walk collected that nothing resolves. The reported list is as long as the template has
/// references, and a reference name inside a stored value is not held to
/// `TemplateParameterName`'s length at all, so this list can run far past the message.
#[tokio::test]
async fn an_unresolved_reference_refusal_is_bounded_however_many_names_it_reports() {
    let emr = Emr::new();
    let cluster = emr
        .cluster("bound-unresolved-host", "tok-bound-unresolved-host")
        .await;
    let suffix = "] that have no value: specify them in jobTemplateParameters or declare a \
                  defaultValue for them.";
    // The wording quotes the template's own id, and every id this service mints is one
    // fixed width, so a stored probe is what measures the wording before a row's
    // reference names are sized to it. Each row then re-asserts the wording its OWN
    // template produced.
    let probe = stored_template(
        &emr,
        "bound-unresolved-probe",
        "tok-bound-unresolved-probe",
        template_data(None),
    )
    .await;
    let room = room_for_the_value(
        format!("Job template {probe} references parameter(s) [")
            .chars()
            .count()
            + suffix.chars().count(),
    );

    for (index, row) in name_list_rows(room).into_iter().enumerate() {
        let case = format!("unresolved references: {}", row.label);
        // The references live in the template's own stored value: `sparkSubmitParameters`
        // admits 102400 characters (smithy_query: shape `SparkSubmitParameters`), so a
        // whole row's worth of them fits in one template.
        let references = row
            .names
            .iter()
            .map(|name| format!("--conf {name}=${{{name}}}"))
            .collect::<Vec<String>>()
            .join(" ");
        let template = stored_template(
            &emr,
            &format!("bound-unresolved-{index}"),
            &format!("tok-bound-unresolved-{index}"),
            template_data(Some(references)),
        )
        .await;
        assert_eq!(
            template.chars().count(),
            probe.chars().count(),
            "case {case:?}: expected every minted id to be the width the probe {probe:?} \
             measured, got {template:?}"
        );
        let prefix = format!("Job template {template} references parameter(s) [");
        let answer = templated_run(
            &emr,
            &cluster,
            &format!("tok-bound-unresolved-{index}"),
            &template,
            None,
        )
        .await;
        answer.refused(&case, 400, INVALID_REQUEST);
        let message = text(&answer.body, "/message", &case);
        assert_bounded(
            &case,
            &message,
            &prefix,
            suffix,
            &row.names.join(SEPARATOR),
        );
    }

    assert_nothing_started(&emr, &cluster, "the unresolved-reference table's cluster").await;
}
