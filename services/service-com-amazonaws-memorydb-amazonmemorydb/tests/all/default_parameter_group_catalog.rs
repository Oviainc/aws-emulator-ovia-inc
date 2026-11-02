// SPDX-License-Identifier: CC0-1.0

//! The parameter groups MemoryDB itself provides in every account, end to end over the
//! wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rule under test: an account already holds one default parameter group per
//! published parameter group family AND the vector-search baseline of every engine line
//! that has one. The two kinds differ in what they report, which is the whole point of
//! the table below: a family's own group reports that family and describes the baseline
//! it is of, while a search baseline is named after its line with `.search`, reports the
//! LINE's family — so a default group's family does not follow from its name — and
//! describes itself as a CUSTOMIZED default group of that family with the vector-search
//! setting on; and the search PREVIEW line is a family of its own. Each group also
//! reports an ARN scoped to the region the call was signed for, and its family's engine
//! defaults: exactly what a group a caller creates in that family starts from.
//!
//! The rows differ along what the rule generalises over: the engine (Redis OSS and
//! Valkey), the major version (6 and 7), and the kind of family (an engine line, a
//! search baseline of a line, and a search-preview family of its own). Source: the
//! `default.*` groups the recorded MemoryDB endpoint reports for an account that created
//! none, and `crate::state::default_parameter_groups`, which derives them from the one
//! engine catalog.

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::PARAMETER_PAGE_SIZE_WINDOW;

use crate::cluster_lifecycle::{ok, router};

/// The region the suite's requests are signed for (the shared harness signs for it), so
/// the ARN a default group reports must carry it rather than a baked-in region.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Every default parameter group an account holds, in name order, as
/// `(group name, the family it reports, the Description it reports)`.
///
/// Spelled out HERE rather than taken from the crate under test, so the assertions are
/// independent of the code that materialises the groups.
const DEFAULT_GROUPS: &[(&str, &str, &str)] = &[
    (
        "default.memorydb-redis6",
        "memorydb_redis6",
        "Default parameter group for memorydb_redis6",
    ),
    (
        "default.memorydb-redis7",
        "memorydb_redis7",
        "Default parameter group for memorydb_redis7",
    ),
    // The search baseline of the Redis OSS 7 line: its own name, its line's family, and
    // the customized-default description the service gives a search baseline.
    (
        "default.memorydb-redis7.search",
        "memorydb_redis7",
        "Customized default parameter group for memorydb_redis7 with search-enabled yes",
    ),
    // The search PREVIEW line, which is a family in its own right and so takes the
    // family form of the description.
    (
        "default.memorydb-redis7.search.preview",
        "memorydb_redis7.search.preview",
        "Default parameter group for memorydb_redis7.search.preview",
    ),
    (
        "default.memorydb-valkey7",
        "memorydb_valkey7",
        "Default parameter group for memorydb_valkey7",
    ),
    // The search baseline of the Valkey 7 line, described like the Redis one.
    (
        "default.memorydb-valkey7.search",
        "memorydb_valkey7",
        "Customized default parameter group for memorydb_valkey7 with search-enabled yes",
    ),
];

/// A string member of a reported group, or a panic naming the case.
fn member(group: &Value, name: &str, case: &str) -> String {
    group
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {name} in {group}"))
        .to_string()
}

/// The `Name`s of the parameters `group` reports, in answer order.
async fn parameter_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    group: &str,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "DescribeParameters",
        // A page size inside the window the parameter-group surface serves, so the read
        // is served rather than refused; the `page_size_window` suite asserts that
        // window.
        &json!({ "ParameterGroupName": group, "MaxResults": PARAMETER_PAGE_SIZE_WINDOW.max }),
        case,
    )
    .await;
    body["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Parameters in {body}"))
        .iter()
        .map(|parameter| member(parameter, "Name", case))
        .collect()
}

/// A store nobody created a group in reports EXACTLY the default groups, in name order,
/// each with the family it carries the engine defaults of.
#[tokio::test]
async fn an_untouched_account_reports_exactly_the_default_parameter_groups() {
    let router = router();
    let case = "untouched store";

    let listed = ok(
        &router,
        "DescribeParameterGroups",
        &json!({ "MaxResults": PARAMETER_PAGE_SIZE_WINDOW.max }),
        case,
    )
    .await;
    let reported: Vec<(String, String)> = listed["ParameterGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ParameterGroups in {listed}"))
        .iter()
        .map(|group| (member(group, "Name", case), member(group, "Family", case)))
        .collect();
    let want: Vec<(String, String)> = DEFAULT_GROUPS
        .iter()
        .map(|(name, family, _)| (name.to_string(), family.to_string()))
        .collect();
    assert_eq!(
        reported, want,
        "case {case:?}: expected exactly the default groups in name order, got {listed}"
    );
}

/// Each default group answers for ITSELF: read one at a time it reports the same family,
/// the description the service gives a baseline of its kind and an ARN in the region the
/// call was signed for — and the parameters it reports are the ones a group a caller
/// creates in that family starts from.
#[tokio::test]
async fn every_default_group_reports_its_family_and_that_family_s_engine_defaults() {
    let router = router();

    for (index, (name, family, description)) in DEFAULT_GROUPS.iter().enumerate() {
        let case = format!("default group {name}");
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
            &case,
        )
        .await;
        let group = &described["ParameterGroups"][0];
        for (reported, expected) in [
            ("Name", json!(name)),
            ("Family", json!(family)),
            ("Description", json!(description)),
            (
                "ARN",
                json!(format!(
                    "arn:aws:memorydb:{REGION}:{ACCOUNT}:parametergroup/{name}"
                )),
            ),
        ] {
            assert_eq!(
                group[reported], expected,
                "case {case:?}: expected {reported} to be {expected}, got {described}"
            );
        }

        let baseline_parameters = parameter_names(&router, name, &case).await;
        assert!(
            !baseline_parameters.is_empty(),
            "case {case:?}: a default group reports its family's engine defaults, got none"
        );
        let mut sorted = baseline_parameters.clone();
        sorted.sort();
        assert_eq!(
            baseline_parameters, sorted,
            "case {case:?}: the baseline must be listed in name order, got \
             {baseline_parameters:?}"
        );

        // A group a caller creates in the SAME family starts from that baseline, which is
        // what makes the family a group reports the family whose defaults it carries.
        let custom = format!("custom-group-{index}");
        ok(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": custom, "Family": family }),
            &case,
        )
        .await;
        let fresh = parameter_names(&router, &custom, &case).await;
        assert_eq!(
            fresh, baseline_parameters,
            "case {case:?}: a group created in {family} must start from the engine \
             defaults the default group reports"
        );

        // And reading the default group changed nothing a later caller sees.
        let again = parameter_names(&router, name, &case).await;
        assert_eq!(
            again, baseline_parameters,
            "case {case:?}: reading the baseline must not change it"
        );
    }
}
