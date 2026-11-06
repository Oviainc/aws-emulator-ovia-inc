// SPDX-License-Identifier: CC0-1.0

//! How `CreateDeployment` words the `ValidationException` it refuses a request with,
//! driven over HTTP through the same router the production emulator serves with.
//!
//! Two rules, each asserted over a TABLE spanning the class rather than over one
//! recorded call:
//!
//! 1. the refusal accumulates EVERY finding the request raised into the one message —
//!    the shape carries a `message` and nothing else — as one sentence per finding,
//!    terminated by a period and joined with `", "`, with no count preamble and no
//!    sentence that is not a finding; the findings are reported in the order a setting
//!    the pattern does not define, then the deployment name, then what the pattern's
//!    published schema says about the settings it does define;
//! 2. the deployment NAME is judged by the rule the service applies to that member
//!    ([`NAME_PATTERN`], a leading letter followed by one or more alphanumerics), which
//!    is STRICTER than the `@pattern` the model declares for `DeploymentName`
//!    (`^[A-Za-z0-9_\.-]+$`, `@length(min: 1, max: 50)` —
//!    `smithy_query(query="operation",operation="CreateDeployment")`): a name the model
//!    admits and that rule refuses is refused, with the finding naming the member and
//!    quoting the expression, and nothing is stored.
//!
//! The rows differ along what the rules generalise over: how MANY findings the request
//! raises (one, two, three and more), WHICH checks raised them (a setting the pattern
//! does not define, the name rule, a field the schema marks required), the deployment
//! pattern they were judged against (one per workload the catalogue holds), and — for
//! the name rule — each character class the two patterns disagree about (hyphen,
//! underscore, dot, a leading digit), each bound of the name's modelled length and the
//! shortest name the rule admits.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::DEPLOYMENT_NAME_SERVER_PATTERN;

use crate::deployment_lifecycle::get;
use crate::deployment_update::{
    conforming_specifications, error_message, one_pattern_per_workload, pattern_data,
    required_field,
};
use crate::harness::{walk_pages, Harness};

/// The wire error code and HTTP status every refusal below carries (`ValidationException`,
/// `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// The expression the service judges a deployment name by, as its finding quotes it: a
/// leading letter followed by one or more alphanumerics. Restated here from the recorded
/// behaviour and pinned to the one the handler enforces by
/// [`the_name_rule_the_handler_enforces_is_the_one_its_finding_quotes`].
const NAME_PATTERN: &str = "^[a-zA-Z][a-zA-Z0-9]+$";

/// The modelled ceiling of `DeploymentName` (`@length(min: 1, max: 50)`), which the
/// server-side rule narrows but does not move.
const MAX_NAME_LEN: usize = 50;

/// A value for a setting the pattern does not publish. `DeploymentSpecifications` is
/// `@sensitive`, so no finding may echo it.
const UNPUBLISHED_VALUE: &str = "unexpected-setting-value";

/// The finding a submitted specification key the pattern does not define raises: fixed
/// text around the key the request carried.
fn disallowed_specification(key: &str) -> String {
    format!("Specification {key} is not allowed.")
}

/// The finding a deployment name outside [`NAME_PATTERN`] raises: fixed text around the
/// member's wire name and the expression it was judged against.
fn name_mismatch() -> String {
    format!(
        "The value of 'deploymentName' doesn't match the regular expression pattern \
         <'{NAME_PATTERN}'>."
    )
}

/// A `CreateDeployment` request body, with the name submitted exactly as given — these
/// rows are about the name rule, so nothing here may repair a name.
fn create_request(
    workload: &str,
    pattern: &str,
    name: &str,
    specifications: &Map<String, Value>,
) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "name": name,
        "specifications": Value::Object(specifications.clone()),
        "dryRun": false,
    })
}

/// Every deployment NAME the service currently stores — what a refused create must not
/// have added to.
async fn stored_names(harness: &Harness, case: &str) -> BTreeSet<String> {
    walk_pages(harness, case, "/listDeployments", &json!({}), "deployments", 100)
        .await
        .iter()
        .map(|deployment| {
            deployment["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a deployment summary carries no name"))
                .to_string()
        })
        .collect()
}

#[test]
fn the_name_rule_the_handler_enforces_is_the_one_its_finding_quotes() {
    // The finding states the expression the name was judged by, so a caller can fix the
    // request from the response alone. This pins the expression these rows assert to the
    // one the handler really applies.
    assert_eq!(
        DEPLOYMENT_NAME_SERVER_PATTERN, NAME_PATTERN,
        "the name rule the handler enforces must be the one its finding quotes, got {DEPLOYMENT_NAME_SERVER_PATTERN:?}"
    );
}

#[tokio::test]
async fn a_deployment_name_is_judged_by_the_rule_the_service_applies_to_it() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);

    // Every name below is inside the MODELLED shape (`^[A-Za-z0-9_\.-]+$`, len 1..50),
    // so what decides each row is the service's own rule and nothing else: the accepted
    // rows span the shortest name it admits, a mixed-case alphanumeric name, one leading
    // capital, and the modelled length ceiling; the refused rows span each character
    // class the two patterns disagree about, a leading digit, and one character — the
    // length the model admits and the rule does not.
    let longest = "n".repeat(MAX_NAME_LEN);
    let rows: Vec<(&str, String, bool)> = vec![
        ("the shortest name the rule admits", "ab".to_string(), true),
        ("a mixed-case alphanumeric name", "sim2realLwName".to_string(), true),
        ("a leading capital", "Deployment9".to_string(), true),
        ("at the modelled length ceiling", longest.clone(), true),
        ("one character, which the rule does not admit", "a".to_string(), false),
        ("a hyphen the model admits", "sim2real-lw-1".to_string(), false),
        ("an underscore the model admits", "deployment_one".to_string(), false),
        ("a dot the model admits", "deployment.one".to_string(), false),
        ("a leading digit", "1deployment".to_string(), false),
        ("a trailing hyphen", "deployment-".to_string(), false),
        (
            "every character class the model admits and the rule does not",
            "AB.cd-ef_gh".to_string(),
            false,
        ),
    ];

    for (label, name, accepted) in rows {
        let case = format!("{case}: {label}");
        let request = create_request(&workload, &pattern, &name, &conforming);
        if accepted {
            // A name the rule admits is stored, and read back through GetDeployment as
            // submitted: the rule refuses names, it does not rewrite them.
            let created = harness.ok(&case, "/createDeployment", request).await;
            let id = created["deploymentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {created}"))
                .to_string();
            let deployment = get(&harness, &case, &id).await;
            assert_eq!(
                deployment["name"],
                json!(name),
                "case {case:?}: a name the rule admits must come back as stored, got {}",
                deployment["name"]
            );
            continue;
        }

        let before = stored_names(&harness, &case).await;
        let body = harness
            .refused(&case, "/createDeployment", request, STATUS, CODE)
            .await;
        let message = error_message(&body);
        assert_eq!(
            message,
            name_mismatch(),
            "case {case:?}: a name only the rule refuses must raise exactly its finding, got \
             {message:?}"
        );
        assert!(
            // A one- or two-character name occurs in ordinary English by accident, so
            // the echo probe is only meaningful for a longer one.
            name.len() < 3 || !message.contains(name.as_str()),
            "case {case:?}: the finding states the member and the rule, not the rejected name, \
             got {message:?}"
        );
        assert_eq!(
            stored_names(&harness, &case).await,
            before,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// One row of the report table: a request that must be refused, and the findings its
/// message must state, in that order.
struct Row {
    label: String,
    /// The deployment name the request carries — one the rule accepts wherever the row
    /// is not about the name rule, so each row states exactly the findings it names.
    name: String,
    specifications: Map<String, Value>,
    findings: Vec<String>,
}

#[tokio::test]
async fn every_finding_a_create_raises_is_accumulated_into_the_one_message() {
    let harness = Harness::new();
    // One row group per workload, so the report is driven over more than one published
    // specification schema.
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let conforming = conforming_specifications(&case, &data, true);
        let required = required_field(&case, &data);

        // A conforming map plus `extra` well-shaped keys the pattern does not define:
        // one finding per key, so a row can ask for any number of them.
        let unpublished = |extra: usize| {
            let mut specifications = conforming.clone();
            for index in 0..extra {
                specifications.insert(format!("NoSuchSetting{index}"), json!(UNPUBLISHED_VALUE));
            }
            specifications
        };
        // The keys above, as findings, in the order the submitted map holds them.
        let disallowed = |extra: usize| -> Vec<String> {
            (0..extra)
                .map(|index| disallowed_specification(&format!("NoSuchSetting{index}")))
                .collect()
        };

        // The name is a conforming one wherever the row is not about the name rule, so
        // each row states exactly the findings it names.
        let good_name = "reportRow";
        let bad_name = "report-row";
        let mut rows: Vec<Row> = Vec::new();
        for extra in 1..=3 {
            rows.push(Row {
                label: format!("{extra} settings the pattern does not define"),
                name: format!("{good_name}{extra}"),
                specifications: unpublished(extra),
                findings: disallowed(extra),
            });
        }
        rows.push(Row {
            label: "a name the rule refuses, settings it accepts".to_string(),
            name: bad_name.to_string(),
            specifications: conforming.clone(),
            findings: vec![name_mismatch()],
        });
        rows.push(Row {
            label: "a setting the pattern does not define and a name the rule refuses"
                .to_string(),
            name: bad_name.to_string(),
            specifications: unpublished(1),
            findings: [disallowed(1), vec![name_mismatch()]].concat(),
        });
        {
            // The recorded shape: the disallowed setting, then the name, then what the
            // published schema asks for — a required field the map no longer carries.
            let mut specifications = unpublished(2);
            specifications.remove(&required);
            rows.push(Row {
                label: format!(
                    "two settings the pattern does not define, a name the rule refuses, and \
                     the required {required} removed"
                ),
                name: bad_name.to_string(),
                specifications,
                findings: [
                    disallowed(2),
                    vec![name_mismatch()],
                    vec![format!(
                        "'{required}' is required by deployment pattern '{pattern}'."
                    )],
                ]
                .concat(),
            });
        }

        for Row {
            label,
            name,
            specifications,
            findings,
        } in rows
        {
            let case = format!("{case}: {label}");
            let before = stored_names(&harness, &case).await;
            let body = harness
                .refused(
                    &case,
                    "/createDeployment",
                    create_request(&workload, &pattern, &name, &specifications),
                    STATUS,
                    CODE,
                )
                .await;
            let message = error_message(&body);
            // Every finding the row raises, in order, each a sentence of its own.
            let expected = findings.join(", ");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the findings {expected:?}, got {message:?}"
            );
            // The envelope: no count preamble, no clause separator other than ", ", and
            // no sentence that is not a finding.
            assert!(
                !message.contains("validation error") && !message.contains("detected: "),
                "case {case:?}: a finding list states no count preamble, got {message:?}"
            );
            assert!(
                !message.contains("; "),
                "case {case:?}: findings are joined with \", \", got {message:?}"
            );
            assert!(
                message.ends_with('.'),
                "case {case:?}: every finding is terminated by a period, got {message:?}"
            );
            assert!(
                !message.contains("GetWorkloadDeploymentPattern"),
                "case {case:?}: the message carries findings only, got {message:?}"
            );
            // `DeploymentSpecifications` is `@sensitive`: no submitted value is replayed.
            assert!(
                !message.contains(UNPUBLISHED_VALUE),
                "case {case:?}: a finding must not echo a submitted value, got {message:?}"
            );
            assert_eq!(
                stored_names(&harness, &case).await,
                before,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}
