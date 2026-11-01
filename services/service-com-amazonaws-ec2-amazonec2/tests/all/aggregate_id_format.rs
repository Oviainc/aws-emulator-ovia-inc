// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAggregateIdFormat`: the Region-wide audit of the
//! account's longer-ID (17-character ID) settings, driven over the wire the service
//! actually speaks — `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the aggregate report carries one status per resource type that has
//! a longer-ID setting at all, and a type's status is the setting EVERY principal of the
//! account is on for that type — so it is off as soon as one principal is off — while the
//! single aggregated flag is on only when every one of those statuses is.
//!
//! So the tables below span that class rather than one recorded request: every resource
//! type the model publishes, both values of the setting, each kind of principal an ARN can
//! name, the `all-current` and `all` wildcards of the modify that writes the settings, the
//! types past their opt-in period, and the refusal `DryRun` documents. The aggregate is
//! also checked against what `DescribePrincipalIdFormat` reports per principal, so the two
//! reports of one stored preference cannot disagree.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::error_code;
use super::wire::{child, child_text, document, set_items, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One form field: the key as ec2Query spells it, and its value.
fn param(name: &str, value: &str) -> Param {
    super::wire::param(name, value)
}

/// An IAM ARN of the account this emulator serves, for a `resource` like `user/alice`.
fn iam_arn(resource: &str) -> String {
    format!("arn:aws:iam::{}:{resource}", engine::emulator_account_id())
}

/// The ARN of the root user, the principal every describe reports.
fn root() -> String {
    ec2::state::root_principal_arn()
}

/// The statuses one `<statusSet>` under `node` carries, in the order the body listed them.
fn statuses(case: &str, body: &str, node: roxmltree::Node<'_, '_>) -> Vec<(String, bool)> {
    set_items(node, "statusSet")
        .into_iter()
        .map(|status| {
            // "If a deadline is not yet available for this resource type, this field is
            // not returned" (`IdFormat$Deadline`), and every type this emulator mints ids
            // for is already on longer ids, so no status may carry one.
            assert!(
                child(status, "deadline").is_none(),
                "case {case:?}: no switch-over deadline is available, so none must be \
                 reported: {body}"
            );
            let resource = child_text(status, "resource")
                .unwrap_or_else(|| panic!("case {case:?}: a status with no <resource> in {body}"));
            let reported = child_text(status, "useLongIds")
                .unwrap_or_else(|| panic!("case {case:?}: a status with no <useLongIds> in {body}"));
            let use_long_ids = reported.parse::<bool>().unwrap_or_else(|_| {
                panic!("case {case:?}: <useLongIds> must be a boolean, got {reported:?}")
            });
            (resource, use_long_ids)
        })
        .collect()
}

/// What one `DescribeAggregateIdFormat` call reports: its statuses in body order, and the
/// Region-wide flag beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Aggregate {
    statuses: Vec<(String, bool)>,
    aggregated: bool,
}

/// `DescribeAggregateIdFormat` with `params`, required to succeed.
async fn aggregate(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Aggregate {
    let (status, body) = call(router, "DescribeAggregateIdFormat", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAggregateIdFormat, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let response = parsed.root_element();
    let reported = child_text(response, "useLongIdsAggregated").unwrap_or_else(|| {
        panic!("case {case:?}: no <useLongIdsAggregated> in the body {body}")
    });
    let aggregated = reported.parse::<bool>().unwrap_or_else(|_| {
        panic!("case {case:?}: <useLongIdsAggregated> must be a boolean, got {reported:?}")
    });
    assert!(
        child(response, "statusSet").is_some(),
        "case {case:?}: no <statusSet> in the body {body}"
    );
    Aggregate {
        statuses: statuses(case, &body, response),
        aggregated,
    }
}

/// The aggregate's setting for one resource type.
async fn region_setting(router: &MultiServiceRouter, case: &str, resource: &str) -> bool {
    aggregate(router, case, &[])
        .await
        .statuses
        .into_iter()
        .find(|(reported, _)| reported == resource)
        .map(|(_, use_long_ids)| use_long_ids)
        .unwrap_or_else(|| panic!("case {case:?}: no status was reported for {resource:?}"))
}

/// What `DescribePrincipalIdFormat` reports: every principal's statuses, keyed by ARN.
async fn per_principal(
    router: &MultiServiceRouter,
    case: &str,
) -> BTreeMap<String, Vec<(String, bool)>> {
    let (status, body) = call(router, "DescribePrincipalIdFormat", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePrincipalIdFormat, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let response = parsed.root_element();
    set_items(response, "principalSet")
        .into_iter()
        .map(|principal| {
            let arn = child_text(principal, "arn")
                .unwrap_or_else(|| panic!("case {case:?}: a principal with no <arn> in {body}"));
            (arn, statuses(case, &body, principal))
        })
        .collect()
}

/// One write to seed a store with: the principal, the resource type and the value.
type Write = (String, &'static str, bool);

/// `ModifyIdentityIdFormat`, required to succeed: the write whose effect the aggregate
/// reports.
async fn modify(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    resource: &str,
    use_long_ids: bool,
) {
    let (status, body) = call(
        router,
        "ModifyIdentityIdFormat",
        &[
            param("PrincipalArn", arn),
            param("Resource", resource),
            param("UseLongIds", if use_long_ids { "true" } else { "false" }),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 setting {resource} to {use_long_ids} for {arn}, \
         got {status} with body {body}"
    );
}

/// The resource types the aggregate reports, in catalog order: the list the model
/// publishes (`DescribePrincipalIdFormatRequest$Resources`, the same types
/// `DescribeAggregateIdFormat`'s own documentation names as supporting longer IDs).
fn catalog() -> Vec<String> {
    ec2::state::LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// What the report carries
// ---------------------------------------------------------------------------

/// "This request only returns information about resource types that support longer IDs":
/// one status per type the catalog holds, in catalog order, and an account where nothing
/// has been set reports every one of them on — so the Region is fully opted in.
#[tokio::test]
async fn a_fresh_account_reports_every_type_on_and_the_region_aggregated() {
    let case = "a fresh account";
    let router = emulator();
    let reported = aggregate(&router, case, &[]).await;

    let resources: Vec<String> = reported
        .statuses
        .iter()
        .map(|(resource, _)| resource.clone())
        .collect();
    assert_eq!(
        resources,
        catalog(),
        "case {case:?}: every resource type that has a setting must be reported once, in \
         catalog order"
    );
    for (resource, use_long_ids) in &reported.statuses {
        assert_eq!(
            *use_long_ids,
            ec2::state::DEFAULT_USE_LONG_IDS,
            "case {case:?}: expected {resource} to report {} by default, got {use_long_ids}",
            ec2::state::DEFAULT_USE_LONG_IDS
        );
    }
    assert!(
        reported.aggregated,
        "case {case:?}: every type is on, so the Region must report aggregated on, got {}",
        reported.aggregated
    );
}

// ---------------------------------------------------------------------------
// One principal's setting decides the Region's
// ---------------------------------------------------------------------------

/// The rule: a type's Region-wide status is off as soon as ONE principal is off for it,
/// every other type stays where it was, and the aggregated flag follows the statuses.
///
/// The rows differ along what the rule generalises over: the kind of principal the ARN
/// names (the root user, an IAM user, an IAM role, a user under an IAM path) and the
/// resource type turned off, including the first and the last type of the opt-in list.
#[tokio::test]
async fn one_principal_off_turns_that_type_off_for_the_region() {
    // (case, the principal written, the resource type turned off)
    let rows: [(&str, String, &str); 6] = [
        ("the root user turning vpc off", root(), "vpc"),
        (
            "an IAM user turning the first opt-in type off",
            iam_arn("user/alice"),
            "bundle",
        ),
        (
            "an IAM role turning the last opt-in type off",
            iam_arn("role/ops"),
            "vpn-gateway",
        ),
        (
            "a user under a path turning image off",
            iam_arn("user/division/subdivision/bob"),
            "image",
        ),
        (
            "an IAM user turning dhcp-options off",
            iam_arn("user/carol"),
            "dhcp-options",
        ),
        (
            "an IAM role turning network-interface off",
            iam_arn("role/auditor"),
            "network-interface",
        ),
    ];

    for (case, arn, resource) in rows {
        let router = emulator();
        modify(&router, case, &arn, resource, false).await;

        let reported = aggregate(&router, case, &[]).await;
        for (type_reported, use_long_ids) in &reported.statuses {
            let expected = type_reported != resource;
            assert_eq!(
                *use_long_ids, expected,
                "case {case:?}: expected {type_reported} to report {expected} for the \
                 Region after {arn} turned {resource} off, got {use_long_ids}"
            );
        }
        assert!(
            !reported.aggregated,
            "case {case:?}: {resource} is off, so the Region cannot report aggregated on, \
             got {}",
            reported.aggregated
        );

        // The same principal putting the type back on restores the Region's own status:
        // the aggregate is a function of the stored settings, not of what was once set.
        modify(&router, case, &arn, resource, true).await;
        let restored = aggregate(&router, case, &[]).await;
        assert!(
            restored.aggregated,
            "case {case:?}: every principal is back on, so the Region must report \
             aggregated on, got {}",
            restored.aggregated
        );
        assert!(
            restored
                .statuses
                .iter()
                .all(|(_, use_long_ids)| *use_long_ids),
            "case {case:?}: every status must be on again, got {:?}",
            restored.statuses
        );
    }
}

/// "This value is only true if all users are configured to use longer IDs": the Region's
/// status for a type is the CONJUNCTION over the account's principals, so the root user
/// being on does not make the Region on while another principal is off — and the aggregate
/// agrees with what `DescribePrincipalIdFormat` reports per principal.
///
/// The rows differ along the stored state the conjunction is taken over: nothing written,
/// one principal off, several principals off for different types, a principal off for a
/// type the root user is off for too, and a principal that overrode a type back ON while
/// the root user has it off.
#[tokio::test]
async fn the_region_setting_is_what_every_principal_agrees_on() {
    // (case, the writes to seed, as (principal, resource, value))
    let rows: [(&str, Vec<Write>); 5] = [
        ("nothing written", Vec::new()),
        (
            "one principal off for one type",
            vec![(iam_arn("user/dave"), "subnet", false)],
        ),
        (
            "two principals off for different types",
            vec![
                (iam_arn("user/erin"), "vpc", false),
                (iam_arn("role/admin"), "flow-log", false),
            ],
        ),
        (
            "the root user and a user off for the same type",
            vec![
                (root(), "prefix-list", false),
                (iam_arn("user/frank"), "prefix-list", false),
            ],
        ),
        (
            "a user back on where the root user is off",
            vec![
                (root(), "export-task", false),
                (iam_arn("user/gina"), "export-task", true),
            ],
        ),
    ];

    for (case, writes) in rows {
        let router = emulator();
        for (arn, resource, use_long_ids) in &writes {
            modify(&router, case, arn, resource, *use_long_ids).await;
        }

        let principals = per_principal(&router, case).await;
        let reported = aggregate(&router, case, &[]).await;
        for (resource, use_long_ids) in &reported.statuses {
            let expected = principals.values().all(|statuses| {
                statuses
                    .iter()
                    .find(|(reported, _)| reported == resource)
                    .map(|(_, value)| *value)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a principal reported no status for {resource:?}")
                    })
            });
            assert_eq!(
                *use_long_ids, expected,
                "case {case:?}: the Region's {resource} must be what every principal \
                 agrees on ({expected}), got {use_long_ids} over {principals:?}"
            );
        }
        let expected = reported
            .statuses
            .iter()
            .all(|(_, use_long_ids)| *use_long_ids);
        assert_eq!(
            reported.aggregated, expected,
            "case {case:?}: the aggregated flag must be on exactly when every status is \
             ({expected}), got {} over {:?}",
            reported.aggregated, reported.statuses
        );
    }
}

/// `all` modifies "the ID format for all IAM users, IAM roles, and the root user of the
/// account": it reaches the principals that had overridden the type, so the Region reports
/// the type on again even though a principal had turned it off.
#[tokio::test]
async fn a_modify_of_all_principals_brings_the_region_back_on() {
    let case = "every principal set back on";
    let router = emulator();
    modify(&router, case, &iam_arn("user/hugo"), "route-table", false).await;
    assert!(
        !region_setting(&router, case, "route-table").await,
        "case {case:?}: one principal is off, so the Region must report route-table off"
    );

    modify(&router, case, ec2::state::ALL_PRINCIPALS, "route-table", true).await;
    let reported = aggregate(&router, case, &[]).await;
    assert!(
        reported.aggregated,
        "case {case:?}: every principal was set on, so the Region must report aggregated \
         on, got {}",
        reported.aggregated
    );
}

/// `all-current` "include[s] all resource types that are currently within their opt-in
/// period for longer IDs": turning it off for one principal turns exactly those types off
/// for the Region, and leaves the types past that period on.
#[tokio::test]
async fn all_current_off_turns_off_exactly_the_opt_in_types_for_the_region() {
    let case = "all-current turned off for one user";
    let router = emulator();
    modify(
        &router,
        case,
        &iam_arn("user/ivy"),
        ec2::state::ALL_CURRENT_RESOURCES,
        false,
    )
    .await;

    let opt_in: BTreeSet<&str> = ec2::state::OPT_IN_LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .collect();
    let reported = aggregate(&router, case, &[]).await;
    for (resource, use_long_ids) in &reported.statuses {
        let expected = !opt_in.contains(resource.as_str());
        assert_eq!(
            *use_long_ids, expected,
            "case {case:?}: expected the Region's {resource} to report {expected} after \
             all-current was turned off, got {use_long_ids}"
        );
    }
    assert!(
        !reported.aggregated,
        "case {case:?}: the opt-in types are off, so the Region cannot report aggregated \
         on, got {}",
        reported.aggregated
    );
}

/// A type past its opt-in period is "permanently switched over to using longer IDs", so no
/// request can turn it off and the Region always reports it on.
///
/// The rows are every such type, so neither the refusal nor the status is keyed to one of
/// them.
#[tokio::test]
async fn a_permanent_type_always_reports_on_for_the_region() {
    let router = emulator();
    for resource in ec2::state::PERMANENT_LONGER_ID_RESOURCE_TYPES {
        let case = format!("turning {resource} off");
        let (status, body) = call(
            &router,
            "ModifyIdentityIdFormat",
            &[
                param("PrincipalArn", &root()),
                param("Resource", resource),
                param("UseLongIds", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
            "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN
        );
        let reported = region_setting(&router, &case, resource).await;
        assert!(
            reported,
            "case {case:?}: {resource} is permanently on longer IDs, so the Region must \
             report it on, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// A read changes nothing
// ---------------------------------------------------------------------------

/// `DescribeAggregateIdFormat` is a REPORT, so reading cannot change what a later caller
/// sees: a run of reads with no writer between them all answer the same thing, and the
/// per-principal report is the same after them as before.
///
/// The rows are a store nothing has written and one several writers have, so the invariant
/// is checked from more than the default.
#[tokio::test]
async fn reading_the_aggregate_never_changes_it() {
    /// Reads per store — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    for seeded in [false, true] {
        let case = format!("repeated reads, seeded {seeded}");
        let router = emulator();
        if seeded {
            modify(&router, &case, &iam_arn("user/jane"), "vpc", false).await;
            modify(&router, &case, &root(), "image", false).await;
        }
        let before = per_principal(&router, &case).await;
        let first = aggregate(&router, &case, &[]).await;
        for read in 1..=READS {
            let again = aggregate(&router, &case, &[]).await;
            assert_eq!(
                again, first,
                "case {case:?}: read {read} must report what the first read did"
            );
        }
        let after = per_principal(&router, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: reading the aggregate must leave the stored settings exactly \
             as they were"
        );
    }
}

// ---------------------------------------------------------------------------
// The documented refusal
// ---------------------------------------------------------------------------

/// "If you have the required permissions, the error response is DryRunOperation" (`DryRun`)
/// — every caller of this emulator holds them, so a dry run is refused with that code and
/// the HTTP 412 EC2 answers it with, and reports nothing. It is not a 501, which would say
/// the operation is unimplemented, and it writes nothing either.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation_and_writes_nothing() {
    let case = "a dry run that would have succeeded";
    let router = emulator();
    modify(&router, case, &iam_arn("user/kate"), "vpc", false).await;
    let before = aggregate(&router, case, &[]).await;

    let (status, body) = call(
        &router,
        "DescribeAggregateIdFormat",
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::DRY_RUN_OPERATION,
        "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
        ec2::state::DRY_RUN_OPERATION
    );
    let after = aggregate(&router, case, &[]).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused dry run must leave the settings exactly as they were"
    );
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally and
/// reports the Region.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_report() {
    let case = "DryRun=false";
    let router = emulator();
    let reported = aggregate(&router, case, &[param("DryRun", "false")]).await;
    assert_eq!(
        reported.statuses.len(),
        catalog().len(),
        "case {case:?}: the report must run and carry one status per resource type, got \
         {:?}",
        reported.statuses
    );
    assert!(
        reported.aggregated,
        "case {case:?}: nothing has been turned off, so the Region must report aggregated \
         on, got {}",
        reported.aggregated
    );
}
