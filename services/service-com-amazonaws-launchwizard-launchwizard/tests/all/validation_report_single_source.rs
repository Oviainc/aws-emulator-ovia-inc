// SPDX-License-Identifier: CC0-1.0

//! That this service states a COUNTED refusal report in ONE sentence, driven over HTTP
//! through the same router the production emulator serves with.
//!
//! RULE: a refusal that counts what failed is stated as `"<count> validation
//! <noun>[s] detected: "` followed by the clauses joined with `"; "` — the count is HOW
//! MANY clauses follow, the noun is singular for one clause and takes an `s` for any
//! other number, and the separator is the same whichever check raised the clauses. The
//! two refusal families this crate counts — a modelled constraint the request broke, and
//! the settings an UPDATE is warned about and may apply anyway — are read the same way by
//! a caller, so they state the same sentence. (A `CreateDeployment` refused for its own
//! input is NOT one of them: it accumulates every finding it raised into AWS's
//! finding list instead, which `tests/all/create_deployment_finding_report.rs` drives.)
//!
//! The suite exists because that sentence used to be rendered in two places in
//! `src/state.rs`, one per family, each with its own count-and-pluralise branch: the
//! rows below drive BOTH families through the operations that reach them, so the two can
//! no longer answer different report shapes.
//!
//! The rows differ along what the rule generalises over: which family raised the clauses
//! (a modelled member constraint, a map KEY outside its modelled shape, a half-specified
//! filter pair, a setting the published schema rejects), which operation answers them
//! (`GetDeployment`, `ListDeployments`, `CreateDeployment`, `UpdateDeployment`), the noun
//! the clauses are counted by (`error` and `warning`), and HOW MANY clauses the report
//! states (one — the singular — and two and three). What each refusal appends AFTER the
//! sentence is read out of the renderer rather than restated here: this suite is about
//! the sentence.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::specification_warning_refusal;

use crate::deployment_lifecycle::create_body;
use crate::deployment_update::{
    conforming_specifications, error_message, one_pattern_per_workload, pattern_data, update_body,
    with_member,
};
use crate::harness::Harness;

/// The wire error code and HTTP status every refusal below carries (`ValidationException`,
/// `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// Long enough for every clock-owed status step a create schedules to come due, so the
/// deployment the update rows name is at rest and takes an update at all. The emulator
/// never sleeps; this is the test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// A value for a setting the pattern does not publish. `DeploymentSpecifications` is
/// `@sensitive`, so no refusal may echo it.
const UNPUBLISHED_VALUE: &str = "unexpected-setting-value";

/// One row: a request that must be refused, and the report sentence it must be refused
/// with.
struct Row {
    case: String,
    /// The request target the refusal answers.
    path: &'static str,
    body: Value,
    /// The noun the report counts its clauses by, in the singular.
    noun: &'static str,
    /// How many clauses the refusal states, by this row's construction.
    clauses: usize,
    /// The fixed sentence this refusal appends after its clauses — empty where it
    /// appends none. See [`trailer`].
    trailer: String,
}

/// The sentence `clauses` clauses of `noun` are stated in: how many, then the clauses.
///
/// `pub(crate)` so the one test-side statement of the sentence is reachable from the
/// other suites in this crate rather than restated in each of them.
pub(crate) fn sentence_prefix(clauses: usize, noun: &str) -> String {
    let counted = if clauses == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    format!("{clauses} validation {counted} detected: ")
}

/// The fixed sentence a refusal appends after its clauses, read out of the renderer
/// itself rather than restated here: ONE synthetic clause rendered, with the report
/// sentence that clause is stated in removed. Doubles as a check that the renderer
/// states a single clause in exactly that sentence.
fn trailer(render: fn(&[String]) -> String, noun: &str) -> String {
    const PROBE: &str = "a-probe-clause";
    let rendered = render(&[PROBE.to_string()]);
    let sentence = format!("{}{PROBE}", sentence_prefix(1, noun));
    rendered
        .strip_prefix(&sentence)
        .unwrap_or_else(|| {
            panic!("case {noun:?}: one clause must be stated as {sentence:?}, got {rendered:?}")
        })
        .to_string()
}

/// Assert `message` states this row's clauses in the one report sentence.
fn assert_report(row: &Row, message: &str) {
    let case = &row.case;
    let prefix = sentence_prefix(row.clauses, row.noun);
    assert!(
        message.starts_with(&prefix),
        "case {case:?}: expected the report to open with {prefix:?}, got {message:?}"
    );
    assert!(
        message.ends_with(row.trailer.as_str()),
        "case {case:?}: expected the report to end with {:?}, got {message:?}",
        row.trailer
    );
    let stated = &message[prefix.len()..message.len() - row.trailer.len()];
    let clauses: Vec<&str> = stated.split("; ").collect();
    assert_eq!(
        clauses.len(),
        row.clauses,
        "case {case:?}: expected {} clauses joined with \"; \", got {clauses:?}",
        row.clauses
    );
    for clause in &clauses {
        assert!(
            !clause.trim().is_empty(),
            "case {case:?}: every clause states something, got {clauses:?}"
        );
    }
    assert!(
        !message.contains(UNPUBLISHED_VALUE),
        "case {case:?}: a report must not echo a submitted value, got {message:?}"
    );
}

#[tokio::test]
async fn every_refusal_states_its_clauses_in_the_one_report_sentence() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let pattern_case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &pattern_case, &workload, &pattern).await;
    let conforming = conforming_specifications(&pattern_case, &data, true);

    // The deployment the `UpdateDeployment` rows name, created first so its clock-owed
    // steps come due while the rows above it run.
    let subject = harness
        .ok(
            &pattern_case,
            "/createDeployment",
            create_body(&harness, &workload, &pattern, "report-sentence", false).await,
        )
        .await["deploymentId"]
        .as_str()
        .expect("a created deployment reports its id")
        .to_string();

    // A `specifications` map carrying `extra` well-shaped keys the pattern does not
    // publish: one clause per key, so a row can ask for any number of clauses.
    let unpublished = |extra: usize| {
        let mut specifications = conforming.clone();
        for index in 0..extra {
            specifications.insert(format!("NoSuchSetting{index}"), json!(UNPUBLISHED_VALUE));
        }
        specifications
    };

    let mut rows: Vec<Row> = Vec::new();

    // Family 1: the clauses a MODELLED constraint raises, through the three ways this
    // crate reaches that renderer — the boundary walk, the filter-pair rule, and a map
    // key judged against the shape the model gives it.
    rows.push(Row {
        case: "a required member is absent".to_string(),
        path: "/getDeployment",
        body: json!({}),
        noun: "error",
        clauses: 1,
        trailer: String::new(),
    });
    rows.push(Row {
        case: "a filter naming a key with no values member".to_string(),
        path: "/listDeployments",
        body: json!({"filters": [{"name": "WORKLOAD_NAME"}]}),
        noun: "error",
        clauses: 1,
        trailer: String::new(),
    });
    rows.push(Row {
        case: "a filter carrying neither half of the pair".to_string(),
        path: "/listDeployments",
        body: json!({"filters": [{}]}),
        noun: "error",
        clauses: 2,
        trailer: String::new(),
    });
    rows.push(Row {
        case: "two filters, each missing a different half".to_string(),
        path: "/listDeployments",
        body: json!({"filters": [{"name": "WORKLOAD_NAME"}, {"values": ["COMPLETED"]}]}),
        noun: "error",
        clauses: 2,
        trailer: String::new(),
    });
    {
        // A `DeploymentSpecifications` key below its modelled length floor
        // (`KeyString` is @length 3..256) — judged in the handler, worded by the same
        // renderer as the boundary rows above.
        let mut specifications = conforming.clone();
        specifications.insert("k".to_string(), json!("x"));
        rows.push(Row {
            case: "a specification key below its modelled length floor".to_string(),
            path: "/createDeployment",
            body: with_member(
                &create_body(&harness, &workload, &pattern, "report-key", false).await,
                "specifications",
                Value::Object(specifications),
            ),
            noun: "error",
            clauses: 1,
            trailer: String::new(),
        });
    }

    // Family 2: the clauses the pattern's PUBLISHED specification schema raises, which an
    // update states as the warnings `force` overrides. Driven below, after the rows above
    // have given the subject's clock-owed steps time to come due.

    for row in &rows {
        let body = harness
            .refused(&row.case, row.path, row.body.clone(), STATUS, CODE)
            .await;
        assert_report(row, &error_message(&body));
    }

    // One row per clause count, so the singular/plural rule is driven on this family and
    // on a noun the error rows never state.
    std::thread::sleep(CREATE_SETTLE);
    let warning_trailer = trailer(specification_warning_refusal, "warning");
    for extra in 1..=3 {
        let row = Row {
            case: format!("an update warned about {extra} unpublished settings"),
            path: "/updateDeployment",
            body: update_body(&subject, &unpublished(extra)),
            noun: "warning",
            clauses: extra,
            trailer: warning_trailer.clone(),
        };
        let body = harness
            .refused(&row.case, row.path, row.body.clone(), STATUS, CODE)
            .await;
        assert_report(&row, &error_message(&body));
    }
}
