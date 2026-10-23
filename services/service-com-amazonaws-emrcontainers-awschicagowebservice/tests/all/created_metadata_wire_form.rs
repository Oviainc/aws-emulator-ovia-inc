// SPDX-License-Identifier: CC0-1.0

//! The creation metadata EMR on EKS reports on a resource, and the wire form it reports
//! it in.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. Every member this service's model targets at `Date` — the `timestamp` shape carrying
//!    `@timestampFormat("date-time")` — goes onto the restJson1 wire as an RFC 3339
//!    date-time STRING, never as an epoch number, on every path that reports one. The rows
//!    span the member (`createdAt`, `finishedAt`), the shape that carries it (`JobTemplate`,
//!    `SecurityConfiguration`, `VirtualCluster`, `JobRun`, `CreateJobTemplateResponse`) and
//!    the kind of path it travels (a create's own payload, a describe's single resource, a
//!    list element).
//! 2. A resource whose model declares `createdBy` reports the IAM principal of the caller
//!    that created it: a `RequestIdentityUserArn`-shaped ARN, the same identity this
//!    emulator's `sts:GetCallerIdentity` publishes, recorded once at create time and
//!    reported identically by every later read.
//! 3. The `containerProvider` a `CreateSecurityConfiguration` carried is echoed back by
//!    `DescribeSecurityConfiguration`, exactly as submitted, and nothing is reported when
//!    the create carried none.

use std::collections::BTreeSet;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr, REGION};

/// `RequestIdentityUserArn`'s modelled `@pattern`
/// `^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$`, length 20..2048
/// (`smithy_query(query="shape", shape="com.amazonaws.emrcontainers#RequestIdentityUserArn")`).
///
/// Only the `aws` partition is admitted here because that is the partition every ARN this
/// emulator mints names; the pattern's `aws[a-zA-Z0-9-]*` alternative is wider.
fn matches_request_identity_arn(arn: &str) -> bool {
    let Some(rest) = arn
        .strip_prefix("arn:aws:iam::")
        .or_else(|| arn.strip_prefix("arn:aws:sts::"))
    else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let account_ok =
        account.is_empty() || (account.len() == 12 && account.chars().all(|c| c.is_ascii_digit()));
    account_ok
        && !resource.is_empty()
        && resource.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '+' | '=' | ',' | '.' | '@' | '-')
        })
        && (20..=2048).contains(&arn.len())
}

/// RULE 1 for ONE member: the value at `pointer` is a JSON string an RFC 3339 reader
/// accepts, and the instant it names lies in `[before, after]`.
///
/// The JSON TYPE is asserted first and on its own: a bare epoch number is the exact defect
/// this pins, and a reader that accepted either form would not see it.
fn assert_date_time_string(case: &str, body: &Value, pointer: &str, before: i64, after: i64) {
    let found = field(body, pointer, case);
    assert!(
        found.as_f64().is_none(),
        "case {case:?}: {pointer} is a JSON number ({found}), but Date carries \
         @timestampFormat(\"date-time\") so it must be an RFC 3339 string"
    );
    let rendered = found.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {pointer} is not a JSON string, got {found}");
    });
    // The `Z`-suffixed UTC form restJson1 puts a `date-time` member in.
    assert!(
        rendered.contains('T') && rendered.ends_with('Z'),
        "case {case:?}: {pointer} {rendered:?} is not an RFC 3339 UTC instant"
    );
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {pointer} {rendered:?} does not parse as date-time: {error}")
    });
    let millis = state::millis_of(&parsed);
    assert!(
        millis >= before - 1_000 && millis <= after + 1_000,
        "case {case:?}: {pointer} {rendered:?} names {millis}, outside the window \
         [{before}, {after}]"
    );
}

/// One `CreateJobTemplate` body, enough to be accepted.
fn job_template_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.10.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/main.py"}},
        },
    })
}

/// `POST /jobtemplates`.
async fn create_template(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/jobtemplates", Some(body)).await
}

/// `GET /jobtemplates/{id}`.
async fn describe_template(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/jobtemplates/{id}"), None).await
}

/// `POST /securityconfigurations`.
async fn create_configuration(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/securityconfigurations", Some(body)).await
}

/// `GET /securityconfigurations/{id}`.
async fn describe_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// RULE 1: every reachable `Date` member, on every path that reports one, is an RFC 3339
/// string.
///
/// The rows are collected by exercising each path once and then asserting the SAME rule
/// over all of them, so a path that renders its timestamp differently from its neighbours
/// fails here rather than in whichever suite happens to read it.
#[tokio::test]
async fn every_date_member_is_an_rfc3339_string_on_every_read_path() {
    let emr = Emr::new();
    let before = state::now_millis();

    // A job template: the create's own `createdAt`, the describe's and the list element's.
    let created_template = create_template(&emr, job_template_body("wire-tpl", "wire-tpl-tok"))
        .await
        .ok("a created job template")
        .clone();
    let template_id = text(&created_template, "/id", "a created job template");
    let described_template = describe_template(&emr, &template_id)
        .await
        .ok("a described job template")
        .clone();
    let listed_templates = emr
        .send("GET", "/jobtemplates", None)
        .await
        .ok("the listed job templates")
        .clone();

    // A security configuration: the describe's `createdAt`.
    let created_configuration = create_configuration(
        &emr,
        json!({
            "name": "wire-sc",
            "clientToken": "wire-sc-tok",
            "securityConfigurationData": {},
        }),
    )
    .await
    .ok("a created security configuration")
    .clone();
    let configuration_id = text(
        &created_configuration,
        "/id",
        "a created security configuration",
    );
    let described_configuration = describe_configuration(&emr, &configuration_id)
        .await
        .ok("a described security configuration")
        .clone();

    // A virtual cluster and a job run under it: the cluster's `createdAt`, the run's
    // `createdAt` on both the describe and the list path, and — once the clock has made
    // the cancellation due — the run's `finishedAt`.
    let cluster = emr.cluster("wire-vc", "wire-vc-tok").await;
    let described_cluster = emr
        .send("GET", &format!("/virtualclusters/{cluster}"), None)
        .await
        .ok("a described virtual cluster")
        .clone();
    let started_run = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "clientToken": "wire-job-tok",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/main.py"}},
            })),
        )
        .await
        .ok("a started job run")
        .clone();
    let run_id = text(&started_run, "/id", "a started job run");
    let described_run = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{run_id}"),
            None,
        )
        .await
        .ok("a described job run")
        .clone();
    let listed_runs = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await
        .ok("the listed job runs")
        .clone();
    emr.send(
        "DELETE",
        &format!("/virtualclusters/{cluster}/jobruns/{run_id}"),
        None,
    )
    .await
    .ok("a cancelled job run");
    // The service stores the due instant and never sleeps; this is the test's own clock.
    std::thread::sleep(state::TRANSITION_SPAN + Duration::from_millis(250));
    let finished_run = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{run_id}"),
            None,
        )
        .await
        .ok("a finished job run")
        .clone();

    let after = state::now_millis();

    // One row per (shape, member, path) the rule covers.
    let rows: [(&str, &Value, &str); 9] = [
        (
            "CreateJobTemplateResponse.createdAt",
            &created_template,
            "/createdAt",
        ),
        (
            "JobTemplate.createdAt on the describe path",
            &described_template,
            "/jobTemplate/createdAt",
        ),
        (
            "JobTemplate.createdAt on the list path",
            &listed_templates,
            "/templates/0/createdAt",
        ),
        (
            "SecurityConfiguration.createdAt",
            &described_configuration,
            "/securityConfiguration/createdAt",
        ),
        (
            "VirtualCluster.createdAt",
            &described_cluster,
            "/virtualCluster/createdAt",
        ),
        (
            "JobRun.createdAt on the describe path",
            &described_run,
            "/jobRun/createdAt",
        ),
        (
            "JobRun.createdAt on the list path",
            &listed_runs,
            "/jobRuns/0/createdAt",
        ),
        (
            "JobRun.createdAt once the run has finished",
            &finished_run,
            "/jobRun/createdAt",
        ),
        (
            "JobRun.finishedAt",
            &finished_run,
            "/jobRun/finishedAt",
        ),
    ];

    for (case, body, pointer) in rows {
        assert_date_time_string(case, body, pointer, before, after);
    }
}

/// RULE 2: `createdBy` names the caller principal, on every resource whose model declares
/// the member and on every path that reports that resource.
///
/// The rows differ along the shape that carries the member and the path it travels — a
/// create's payload, a single-resource describe, a list element — and along what the
/// creating request carried beside the members under test.
#[tokio::test]
async fn created_by_names_the_caller_principal_on_every_resource() {
    let emr = Emr::new();
    // The identity this emulator authenticates every caller as, and the one its
    // `sts:GetCallerIdentity` publishes: `arn:aws:iam::<account>:user/emulator`.
    let expected = format!(
        "arn:aws:iam::{}:user/emulator",
        engine::emulator_account_id()
    );
    assert_eq!(
        state::caller_identity_arn(),
        expected,
        "the service's caller principal must be the emulator's published IAM user"
    );

    // Two job templates, differing in what the create carried beside the required
    // members, so `createdBy` is asserted over more than one stored record.
    let mut template_ids = Vec::new();
    for (label, extra) in [
        ("a template with nothing optional", json!({})),
        (
            "a template with tags and a KMS key",
            json!({
                "tags": {"team": "data"},
                "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        ),
    ] {
        let index = template_ids.len();
        // `ResourceNameString` is `^[\.\-_/#A-Za-z0-9]+$`, so the row's prose label names
        // the case in assertions while the request carries a name the model admits.
        let mut body = job_template_body(&format!("by-tpl-{index}"), &format!("by-tok-{index}"));
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let created = create_template(&emr, body).await;
        template_ids.push(text(created.ok(label), "/id", label));
    }

    let configuration = create_configuration(
        &emr,
        json!({
            "name": "by-sc",
            "clientToken": "by-sc-tok",
            "securityConfigurationData": {},
        }),
    )
    .await;
    let configuration_id = text(configuration.ok("a security configuration"), "/id", "by-sc");

    let cluster = emr.cluster("by-vc", "by-vc-tok").await;
    let started = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "clientToken": "by-job-tok",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSqlJobDriver": {"entryPoint": "s3://bucket/q.sql"}},
            })),
        )
        .await;
    let run_id = text(started.ok("a job run"), "/id", "a job run");

    // Each row: the case, the body read, and the pointer at `createdBy`.
    let mut rows: Vec<(String, Value, String)> = Vec::new();
    for (index, id) in template_ids.iter().enumerate() {
        let described = describe_template(&emr, id).await;
        rows.push((
            format!("JobTemplate {index} on the describe path"),
            described.ok("a described job template").clone(),
            "/jobTemplate/createdBy".to_string(),
        ));
    }
    let listed = emr.send("GET", "/jobtemplates", None).await;
    let listed = listed.ok("the listed job templates").clone();
    for index in 0..template_ids.len() {
        rows.push((
            format!("JobTemplate element {index} on the list path"),
            listed.clone(),
            format!("/templates/{index}/createdBy"),
        ));
    }
    let described_configuration = describe_configuration(&emr, &configuration_id).await;
    rows.push((
        "SecurityConfiguration on the describe path".to_string(),
        described_configuration
            .ok("a described security configuration")
            .clone(),
        "/securityConfiguration/createdBy".to_string(),
    ));
    let described_run = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{run_id}"),
            None,
        )
        .await;
    rows.push((
        "JobRun on the describe path".to_string(),
        described_run.ok("a described job run").clone(),
        "/jobRun/createdBy".to_string(),
    ));
    let listed_runs = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    rows.push((
        "JobRun element on the list path".to_string(),
        listed_runs.ok("the listed job runs").clone(),
        "/jobRuns/0/createdBy".to_string(),
    ));

    let mut reported = BTreeSet::new();
    for (case, body, pointer) in &rows {
        let arn = text(body, pointer, case);
        assert!(
            matches_request_identity_arn(&arn),
            "case {case:?}: createdBy {arn:?} is not a RequestIdentityUserArn \
             (^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\\d{{12}})?:[\\w/+=,.@-]+$, len 20..2048)"
        );
        assert_eq!(
            arn, expected,
            "case {case:?}: createdBy must name the caller principal, got {arn:?}"
        );
        reported.insert(arn);
    }
    assert_eq!(
        reported.len(),
        1,
        "every read path must report ONE principal for these resources, got {reported:?}"
    );
    assert_eq!(
        rows.len(),
        7,
        "every createdBy-bearing shape and path must be covered, got {} rows",
        rows.len()
    );
}

/// RULE 3: the `containerProvider` a `CreateSecurityConfiguration` carried is echoed back
/// on read, exactly as submitted, and nothing is reported when the create carried none.
///
/// Recorded behaviour, not modelled: the on-disk `SecurityConfiguration` shape declares no
/// `containerProvider`, while the create's input does accept one and AWS returns it on
/// every read of the configuration. The rows differ along how deep the submitted provider
/// is — absent, the two required members only, with `info.eksInfo.namespace`, with
/// `nodeLabel` beside it — and along the `ClusterId` form (the model's
/// `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, length 1..100, tested at both bounds).
#[tokio::test]
async fn the_create_time_container_provider_is_echoed_on_read() {
    let emr = Emr::new();
    let cases: [(&str, Option<Value>); 5] = [
        ("no container provider at all", None),
        (
            "the two required members only",
            Some(json!({"type": "EKS", "id": "eks-minimal"})),
        ),
        (
            "an eks namespace",
            Some(json!({
                "type": "EKS",
                "id": "eks-ns",
                "info": {"eksInfo": {"namespace": "default"}},
            })),
        ),
        (
            "an eks namespace and a node label",
            Some(json!({
                "type": "EKS",
                "id": "eks_node-label1",
                "info": {"eksInfo": {"namespace": "emr-secure", "nodeLabel": "spark-nodes"}},
            })),
        ),
        (
            // `ClusterId` is length 1..100: a one-character id and a 100-character one.
            "cluster ids at both length bounds",
            Some(json!({
                "type": "EKS",
                "id": format!("a{}", "b".repeat(99)),
                "info": {"eksInfo": {"namespace": "x"}},
            })),
        ),
    ];

    for (index, (label, provider)) in cases.iter().enumerate() {
        let mut body = json!({
            "name": format!("cp-{index}"),
            "clientToken": format!("cp-tok-{index}"),
            "securityConfigurationData": {},
        });
        if let (Some(target), Some(provider)) = (body.as_object_mut(), provider.as_ref()) {
            target.insert("containerProvider".to_string(), provider.clone());
        }
        let created = create_configuration(&emr, body).await;
        let id = text(created.ok(label), "/id", label);

        let described = describe_configuration(&emr, &id).await;
        let configuration = field(described.ok(label), "/securityConfiguration", label).clone();
        assert_eq!(
            configuration.get("containerProvider").cloned(),
            provider.clone(),
            "case {label:?}: the create-time containerProvider must be echoed on read, \
             got {configuration}"
        );

        // Reading it back twice answers the same record: the echo comes out of state, not
        // out of the request that happened to be in flight.
        let again = describe_configuration(&emr, &id).await;
        assert_eq!(
            field(again.ok(label), "/securityConfiguration", label),
            &configuration,
            "case {label:?}: describing a configuration twice answered two different records"
        );

        // The arn is still the one this region's caller signed for, so the extra member
        // did not displace a modelled one.
        assert_eq!(
            text(&configuration, "/arn", label),
            format!(
                "arn:aws:emr-containers:{REGION}:{}:/securityconfigurations/{id}",
                engine::emulator_account_id()
            ),
            "case {label:?}: the described arn is not the one built for this id"
        );
    }
}
