// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `Enable`/`DisableInstanceSqlHaStandbyDetections`, driven over the
//! wire the service actually speaks (`ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. ENABLING REGISTERS THE INSTANCES THE REQUEST NAMES for SQL Server High Availability
//!    standby detection monitoring: each one is reported back in the `processing` status a
//!    fresh registration starts in, with the secret the request named, and
//!    `DescribeInstanceSqlHaStates` — which reads the same stored registration — reports
//!    exactly the same rows. The determination monitoring owes the node falls due one
//!    `TRANSITION_DELAY` later whether or not anybody calls again.
//! 2. DISABLING TAKES THOSE REGISTRATIONS BACK OUT: the instances are reported as the state
//!    monitoring had reached for them, the describe no longer reports them at all, and an
//!    instance nothing had enabled contributes no row — there being no registration of it to
//!    remove.
//! 3. BOTH OPERATIONS REFUSE IN ONE ORDER: an instance id outside the form EC2 issues is
//!    `InvalidInstanceID.Malformed` even under `DryRun`; a `DryRun` whose ids are all readable
//!    is answered 412 `DryRunOperation` before the instances are looked up, so one naming an
//!    id the account holds no instance for still gets that answer; and a well-formed id the
//!    account holds no instance for is `InvalidInstanceID.NotFound` without the flag. Every
//!    refusal leaves the store exactly as it was.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! instances one request names, whether a secret is named, which id form is sent, which of
//! the two operations is called — so no row is the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, HA_STATUS_INVALID, HA_STATUS_PROCESSING,
    SQL_HA_STATUS_UNDETERMINED_DETAIL, SQL_SERVER_LICENSE_USAGE_FULL, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A registration is a fact of the instance rather
/// than of the Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The image every launch here names. Which AMI it is does not matter to a registration.
const AMI: &str = "ami-0123456789abcdef0";

/// One Secrets Manager secret ARN an enablement may name, in the form
/// `EnableInstanceSqlHaStandbyDetectionsRequest$SqlServerCredentials` publishes.
const SECRET: &str =
    "arn:aws:secretsmanager:us-west-2:123456789012:secret:sqlha/cluster-a-AbCdEf";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// Long enough past [`TRANSITION_DELAY`] that a determination the clock owes a registration is
/// certainly due, with a margin so a slow machine cannot read the state before it is.
fn past_the_determination() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One row of a `RegisteredInstance` response set, with the members both operations report.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    instance_id: String,
    ha_status: Option<String>,
    sql_server_license_usage: Option<String>,
    processing_status: Option<String>,
    sql_server_credentials: Option<String>,
}

/// Every `instanceSet/item` a response body carries, in document order.
fn rows(case: &str, body: &str) -> Vec<Row> {
    let parsed = document(case, body);
    let Some(set) = child(parsed.root_element(), "instanceSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Row {
            instance_id: child_text(item, "instanceId").unwrap_or_default(),
            ha_status: child_text(item, "haStatus"),
            sql_server_license_usage: child_text(item, "sqlServerLicenseUsage"),
            processing_status: child_text(item, "processingStatus"),
            sql_server_credentials: child_text(item, "sqlServerCredentials"),
        })
        .collect()
}

/// The emulator and the shared engine its store is read from.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Launch `count` instances and answer their ids, in the order the launch reported them.
async fn launch(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let params = vec![
        p("ImageId", AMI),
        p("MinCount", count.to_string()),
        p("MaxCount", count.to_string()),
    ];
    let (status, body) = call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let ids: Vec<String> = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "instanceId")
        .filter_map(|node| node.text().map(str::to_string))
        .collect();
    assert_eq!(
        ids.len(),
        count,
        "case {case:?}: expected {count} launched instances, got {ids:?} from body {body}"
    );
    ids
}

/// The indexed `InstanceId.<n>` fields naming `ids`.
fn instance_id_params(ids: &[&str]) -> Vec<(String, String)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| p(&format!("InstanceId.{}", index + 1), *id))
        .collect()
}

/// One `EnableInstanceSqlHaStandbyDetections` call naming `ids`, with `extra` fields beside.
async fn enable(
    router: &MultiServiceRouter,
    ids: &[&str],
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = instance_id_params(ids);
    params.extend_from_slice(extra);
    call(
        router,
        REGION,
        "EnableInstanceSqlHaStandbyDetections",
        &params,
    )
    .await
}

/// One `DisableInstanceSqlHaStandbyDetections` call naming `ids`, with `extra` fields beside.
async fn disable(
    router: &MultiServiceRouter,
    ids: &[&str],
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = instance_id_params(ids);
    params.extend_from_slice(extra);
    call(
        router,
        REGION,
        "DisableInstanceSqlHaStandbyDetections",
        &params,
    )
    .await
}

/// What `DescribeInstanceSqlHaStates` reports for the whole account.
async fn described(router: &MultiServiceRouter, case: &str) -> Vec<Row> {
    let (status, body) = call(router, REGION, "DescribeInstanceSqlHaStates", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceSqlHaStates, \
         got {status} with body {body}"
    );
    rows(case, &body)
}

/// How many registrations the store holds, read straight off the state the handlers write.
fn stored_registrations(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().sql_ha_registrations.len()
}

/// RULE 1: an enable registers exactly the instances it names, reports them in `processing`,
/// and a describe reads the same registrations back — with the determination the clock owes
/// them falling due on its own.
#[tokio::test]
async fn enabling_registers_the_named_instances_and_reports_them_as_a_describe_does() {
    // Rows differing along how many instances one request names and whether it names a
    // secret: the registration is per instance, and the secret is per request.
    let rows_under_test: [(&str, usize, Option<&str>); 4] = [
        ("one instance, no secret", 1, None),
        ("one instance with a secret", 1, Some(SECRET)),
        ("three instances with a secret", 3, Some(SECRET)),
        ("five instances, no secret", 5, None),
    ];

    for (case, count, secret) in rows_under_test {
        let (router, engine) = emulator();
        // One more instance than the request names, so "the instances named" is a choice
        // among several rather than everything the account holds.
        let launched = launch(&router, case, count + 1).await;
        let named: Vec<&str> = launched[..count].iter().map(String::as_str).collect();
        let extra: Vec<(String, String)> = secret
            .map(|secret| vec![p("SqlServerCredentials", secret)])
            .unwrap_or_default();

        let (status, body) = enable(&router, &named, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the enable, got {status} with body {body}"
        );
        let reported = rows(case, &body);
        let expected: Vec<Row> = named
            .iter()
            .map(|instance_id| Row {
                instance_id: (*instance_id).to_string(),
                // "Once enabled, Amazon Web Services monitors the metadata for the instances
                // to determine whether they are active or standby nodes", so a fresh
                // registration is still having its status determined.
                ha_status: Some(HA_STATUS_PROCESSING.to_string()),
                sql_server_license_usage: Some(SQL_SERVER_LICENSE_USAGE_FULL.to_string()),
                processing_status: None,
                sql_server_credentials: secret.map(str::to_string),
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the enable to report every instance it registered, \
             got {reported:?} from body {body}"
        );

        // The describe reads the SAME stored registrations: the instance the request did not
        // name is registered nowhere, so it is no row at all.
        let described_now = described(&router, case).await;
        let mut sorted = described_now.clone();
        sorted.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let mut wanted = expected.clone();
        wanted.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        assert_eq!(
            sorted, wanted,
            "case {case:?}: the describe must report the registrations the enable wrote, \
             got {described_now:?}"
        );
        assert_eq!(
            stored_registrations(&engine),
            count,
            "case {case:?}: expected {count} stored registrations"
        );

        // The determination was owed at its instant whether or not anybody called, so once it
        // has passed every registered instance reports the status monitoring reached — and no
        // describe had to write it.
        tokio::time::sleep(past_the_determination()).await;
        let settled = described(&router, case).await;
        let statuses: Vec<Option<String>> = settled.iter().map(|row| row.ha_status.clone()).collect();
        assert_eq!(
            statuses,
            vec![Some(HA_STATUS_INVALID.to_string()); count],
            "case {case:?}: expected every registration's determination to have fallen due, \
             got {settled:?}"
        );
        let details: Vec<Option<String>> = settled
            .iter()
            .map(|row| row.processing_status.clone())
            .collect();
        assert_eq!(
            details,
            vec![Some(SQL_HA_STATUS_UNDETERMINED_DETAIL.to_string()); count],
            "case {case:?}: expected the determination's own detail on every row, \
             got {settled:?}"
        );
    }
}

/// RULE 1, continued: one instance holds one registration however many times a request names
/// it, and re-enabling it replaces the registration rather than adding a second.
#[tokio::test]
async fn one_instance_holds_one_registration() {
    let (router, engine) = emulator();
    let launched = launch(&router, "the registered instance", 1).await;
    let instance_id = launched[0].as_str();

    // Rows differing along how the same instance is named twice: inside one request, and
    // across two requests — with the second naming a secret the first did not.
    let (status, body) = enable(&router, &[instance_id, instance_id], &[]).await;
    assert_eq!(
        status, 200,
        "case \"one id twice in one request\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = rows("one id twice in one request", &body);
    assert_eq!(
        reported.len(),
        1,
        "case \"one id twice in one request\": one instance is one row, got {reported:?}"
    );
    assert_eq!(
        stored_registrations(&engine),
        1,
        "case \"one id twice in one request\": one instance is one registration"
    );

    let (status, body) = enable(&router, &[instance_id], &[p("SqlServerCredentials", SECRET)]).await;
    assert_eq!(
        status, 200,
        "case \"enabled again with a secret\": expected HTTP 200, got {status} with body {body}"
    );
    let described_now = described(&router, "enabled again with a secret").await;
    assert_eq!(
        described_now
            .first()
            .and_then(|row| row.sql_server_credentials.clone()),
        Some(SECRET.to_string()),
        "case \"enabled again with a secret\": the second enablement's secret must be the \
         one the registration reports, got {described_now:?}"
    );
    assert_eq!(
        stored_registrations(&engine),
        1,
        "case \"enabled again with a secret\": re-enabling replaces the registration"
    );
}

/// RULE 2: a disable takes the registration back out — reporting the state monitoring had
/// reached — and an instance nothing had enabled contributes no row.
#[tokio::test]
async fn disabling_removes_the_registration_and_reports_what_it_removed() {
    // Rows differing along which of the registered instances the disable names, and whether
    // the determination has fallen due by then.
    let rows_under_test: [(&str, usize, bool); 4] = [
        ("the first of three, still processing", 0, false),
        ("one in the middle of three, still processing", 1, false),
        ("the last of three, still processing", 2, false),
        ("one of three, after its determination", 1, true),
    ];

    for (case, index, settled) in rows_under_test {
        let (router, engine) = emulator();
        let launched = launch(&router, case, 4).await;
        let registered: Vec<&str> = launched[..3].iter().map(String::as_str).collect();
        let (status, body) = enable(&router, &registered, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the enable must be served, got {status} with body {body}"
        );
        if settled {
            tokio::time::sleep(past_the_determination()).await;
        }

        let named = registered[index];
        let (status, body) = disable(&router, &[named], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the disable, got {status} with body {body}"
        );
        let reported = rows(case, &body);
        let expected_status = if settled {
            HA_STATUS_INVALID
        } else {
            HA_STATUS_PROCESSING
        };
        assert_eq!(
            reported,
            vec![Row {
                instance_id: named.to_string(),
                ha_status: Some(expected_status.to_string()),
                sql_server_license_usage: Some(SQL_SERVER_LICENSE_USAGE_FULL.to_string()),
                processing_status: settled
                    .then(|| SQL_HA_STATUS_UNDETERMINED_DETAIL.to_string()),
                sql_server_credentials: None,
            }],
            "case {case:?}: the disable must report the state the instance had reached, \
             got {reported:?} from body {body}"
        );

        // The registration is gone: the describe reports the other two and not this one.
        let described_now = described(&router, case).await;
        let reported_ids: Vec<String> = described_now
            .iter()
            .map(|row| row.instance_id.clone())
            .collect();
        let still_registered: Vec<String> = registered
            .iter()
            .filter(|instance_id| **instance_id != named)
            .map(|instance_id| (*instance_id).to_string())
            .collect();
        assert_eq!(
            reported_ids, still_registered,
            "case {case:?}: expected only the registrations the disable left, \
             got {described_now:?}"
        );
        assert_eq!(
            stored_registrations(&engine),
            2,
            "case {case:?}: the disable must remove exactly one registration"
        );

        // A second disable of the same instance, and a disable of an instance nothing ever
        // enabled, each have no registration to remove — so each reports no row and removes
        // nothing.
        for (repeat_case, target) in [
            (format!("{case}: disabled again"), named),
            (
                format!("{case}: an instance nothing enabled"),
                launched[3].as_str(),
            ),
        ] {
            let (status, body) = disable(&router, &[target], &[]).await;
            assert_eq!(
                status, 200,
                "case {repeat_case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported = rows(&repeat_case, &body);
            assert!(
                reported.is_empty(),
                "case {repeat_case:?}: expected no row for an instance with no registration, \
                 got {reported:?}"
            );
            assert_eq!(
                stored_registrations(&engine),
                2,
                "case {repeat_case:?}: a disable with nothing to remove must remove nothing"
            );
        }
    }
}

/// RULE 3: both operations read each id's FORM first — refusing a value outside it even under
/// `DryRun` — then answer the permission question, and only then resolve the instances.
#[tokio::test]
async fn an_unreadable_instance_id_is_refused_before_the_permission_check() {
    // The ids of each row are values EC2 never issued an instance id
    // (`ec2::state::instance_id_is_well_formed`): the all-zero placeholder, a long form whose
    // leading digit is above zero, a value of no EC2 id length, one of another resource kind
    // and one without the prefix at all.
    let malformed: [(&str, &str); 5] = [
        ("the all-zero placeholder", "i-00000000000000000"),
        ("a leading digit above zero", "i-1234567890abcdef0"),
        ("a length no EC2 id has", "i-012345"),
        ("another resource kind", "vpc-0123456789abcdef0"),
        ("no resource prefix at all", "0123456789abcdef0"),
    ];

    for action in [
        "EnableInstanceSqlHaStandbyDetections",
        "DisableInstanceSqlHaStandbyDetections",
    ] {
        for (form, id) in malformed {
            for dry_run in [false, true] {
                let (router, engine) = emulator();
                let launched = launch(&router, form, 1).await;
                let case = format!("{action} naming {form} with DryRun={dry_run}");
                // The readable id of a stored instance sits beside the unreadable one, so the
                // refusal is about the FORM rather than about the request naming one id.
                let mut params = instance_id_params(&[launched[0].as_str(), id]);
                if dry_run {
                    params.push(p("DryRun", "true"));
                }
                let (status, body) = call(&router, REGION, action, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInstanceID.Malformed",
                    "case {case:?}: expected the malformed-id code, got body {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("The instance ID '{id}' is malformed"),
                    "case {case:?}: expected this operation's own wording, got body {body}"
                );
                assert_eq!(
                    stored_registrations(&engine),
                    0,
                    "case {case:?}: a refusal must store nothing"
                );
            }
        }
    }
}

/// RULE 3, continued: a `DryRun` whose ids are all readable is answered `DryRunOperation`
/// before the instances are resolved, and a well-formed id the account holds no instance for
/// is refused without the flag.
#[tokio::test]
async fn a_dry_run_is_answered_before_the_instances_are_resolved() {
    // A well-formed long-form id no instance of the account carries: the mint counts from
    // one, so an id far above anything it has reached names nothing.
    const ABSENT: &str = "i-0fedcba987654321f";

    for action in [
        "EnableInstanceSqlHaStandbyDetections",
        "DisableInstanceSqlHaStandbyDetections",
    ] {
        // Rows differing along which instances a readable request names: one the account
        // holds, one it does not, and both at once.
        let rows_under_test: [(&str, bool, bool); 3] = [
            ("a stored instance", true, false),
            ("an id naming no instance", false, true),
            ("both at once", true, true),
        ];
        for (named, include_stored, include_absent) in rows_under_test {
            let (router, engine) = emulator();
            let launched = launch(&router, named, 1).await;
            let mut ids: Vec<&str> = Vec::new();
            if include_stored {
                ids.push(launched[0].as_str());
            }
            if include_absent {
                ids.push(ABSENT);
            }

            let case = format!("{action}: a dry run naming {named}");
            let mut params = instance_id_params(&ids);
            params.push(p("DryRun", "true"));
            let (status, body) = call(&router, REGION, action, &params).await;
            assert_eq!(
                status, 412,
                "case {case:?}: expected HTTP 412, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DryRunOperation",
                "case {case:?}: expected DryRunOperation, got body {body}"
            );
            assert_eq!(
                error_message(&body),
                "Request would have succeeded, but DryRun flag is set.",
                "case {case:?}: expected EC2's own dry-run wording, got body {body}"
            );
            assert_eq!(
                stored_registrations(&engine),
                0,
                "case {case:?}: a dry run must store nothing"
            );

            // Without the flag the same request is served or refused on its own terms: an id
            // naming no instance of the account is not found.
            let params = instance_id_params(&ids);
            let (status, body) = call(&router, REGION, action, &params).await;
            if include_absent {
                assert_eq!(
                    status, 400,
                    "case {case:?} without the flag: expected HTTP 400, \
                     got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInstanceID.NotFound",
                    "case {case:?} without the flag: expected the not-found code, \
                     got body {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("The instance ID '{ABSENT}' does not exist"),
                    "case {case:?} without the flag: expected the not-found wording, \
                     got body {body}"
                );
                assert_eq!(
                    stored_registrations(&engine),
                    0,
                    "case {case:?} without the flag: a refusal must store nothing"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?} without the flag: expected HTTP 200, \
                     got {status} with body {body}"
                );
            }
        }
    }
}

/// RULE 3, continued: a request naming no instance at all asks for nothing, and the boundary
/// validator refuses it against the model's own `@length` on the list.
#[tokio::test]
async fn a_request_naming_no_instance_is_refused() {
    let (router, engine) = emulator();
    let mut seen: BTreeMap<&str, u16> = BTreeMap::new();
    for action in [
        "EnableInstanceSqlHaStandbyDetections",
        "DisableInstanceSqlHaStandbyDetections",
    ] {
        let (status, body) = call(&router, REGION, action, &[]).await;
        assert!(
            (400..500).contains(&status),
            "case {action:?}: a request naming no instance must be refused, \
             got {status} with body {body}"
        );
        seen.insert(action, status);
    }
    assert_eq!(
        stored_registrations(&engine),
        0,
        "a refused request must store nothing: {seen:?}"
    );
}
