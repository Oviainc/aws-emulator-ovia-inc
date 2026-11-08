// SPDX-License-Identifier: CC0-1.0

//! How a Managed Instances launch template's `monitoring` option is SPELLED, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `InstanceLaunchTemplate$monitoring` names one of the two
//! CloudWatch monitoring categories, and the value a request spells it with is matched
//! against the modelled set without regard to ASCII case — the member's own documentation
//! publishes the categories in lower case ("CloudWatch provides two categories of
//! monitoring: basic monitoring and detailed monitoring", `smithy_query` query="doc"
//! shape="InstanceLaunchTemplate" member="monitoring") and Amazon ECS serves a request
//! that spells the option that way — while the stored provider reports the option in the
//! spelling the modelled set lists (`smithy_query` query="shape"
//! shape="ManagedInstancesMonitoringOptions" -> `["BASIC", "DETAILED"]`), so a read
//! carries a value inside that set whichever spelling created it. A value that matches no
//! option in ANY case is outside the set and is refused with HTTP 400 and the modelled
//! `InvalidParameterException`, worded the way every out-of-set enumerated value of this
//! service is worded ([`crate::enum_value_refusals`] is that sentence's subject), and the
//! refused create stores no provider.
//!
//! The rows span the two options, the spelling the set lists, the spelling the
//! documentation publishes, mixed and alternating case, the member left out, and the forms
//! that name no option at all: a truncation, an empty value, a padded option, a value of
//! another enumerated member of the same template, and an unrelated word.
//!
//! What the stored template reports for each option is [`crate::probe_monitoring`]'s
//! subject; this suite is about which SPELLINGS reach it.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::managed_instances_monitored;
use crate::ecs_wire::{create_cluster, list, ok, refusal, service};

/// The cluster every Managed Instances provider below is scoped to.
const CLUSTER: &str = "monitoringcase";

/// The one error code this service answers a value outside an enumerated member's set
/// with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole sentence a `monitoring` outside the modelled set is refused with: the member
/// in Amazon ECS's own words and the values it accepts in ascending order between
/// brackets, the template [`crate::enum_value_refusals`] pins for every enumerated member
/// of this service.
const OUT_OF_SET: &str = "monitoring should be one of [BASIC,DETAILED]";

/// Read one capacity provider back and return the `monitoring` its stored launch template
/// reports, `None` for a template that reports none.
async fn stored_monitoring(
    router: &MultiServiceRouter,
    provider_name: &str,
    case: &str,
) -> Option<String> {
    let body = ok(
        router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": [provider_name] }),
        case,
    )
    .await;
    let described = list(&body, "capacityProviders", case);
    assert_eq!(
        described.len(),
        1,
        "case {case:?}: expected the created provider back, got {body}"
    );
    described[0]["managedInstancesProvider"]["instanceLaunchTemplate"]["monitoring"]
        .as_str()
        .map(str::to_string)
}

#[tokio::test]
async fn a_monitoring_option_is_accepted_in_any_case_and_stored_in_the_modelled_spelling() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    // (case, provider name, the value the request spells `monitoring` with, the option the
    // stored provider must report).
    let rows: [(&str, &str, Option<&str>, Option<&str>); 8] = [
        (
            "the set's own spelling of the first option",
            "setbasic",
            Some("BASIC"),
            Some("BASIC"),
        ),
        (
            "the set's own spelling of the second option",
            "setdetailed",
            Some("DETAILED"),
            Some("DETAILED"),
        ),
        (
            "the lower-case spelling the member's documentation publishes",
            "docbasic",
            Some("basic"),
            Some("BASIC"),
        ),
        (
            "the lower-case spelling of the second option",
            "docdetailed",
            Some("detailed"),
            Some("DETAILED"),
        ),
        (
            "a capitalised spelling",
            "capitalised",
            Some("Basic"),
            Some("BASIC"),
        ),
        (
            "an alternating-case spelling",
            "alternating",
            Some("DeTaIlEd"),
            Some("DETAILED"),
        ),
        (
            "a spelling that lower-cases only the first letters",
            "leadinglower",
            Some("deTAILED"),
            Some("DETAILED"),
        ),
        (
            "the member left out, which names no option to report",
            "omitted",
            None,
            None,
        ),
    ];

    for (case, provider_name, requested, expected) in rows {
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider": managed_instances_monitored(requested),
            }),
            case,
        )
        .await;
        let answered = created["capacityProvider"]["managedInstancesProvider"]
            ["instanceLaunchTemplate"]["monitoring"]
            .as_str();
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the create to answer monitoring {expected:?}, got \
             {answered:?} in {created}"
        );
        let stored = stored_monitoring(&router, provider_name, case).await;
        assert_eq!(
            stored.as_deref(),
            expected,
            "case {case:?}: expected the stored provider to report monitoring {expected:?}, \
             got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_monitoring_value_that_names_no_option_is_refused_and_stores_no_provider() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    // Every form that matches no option in any case: a truncation, an empty value, an
    // option padded with spaces, a value of another enumerated member of the same launch
    // template (`CapacityOptionType`, whose set is ON_DEMAND, SPOT, RESERVED), the two
    // options run together, and a word of neither.
    let rows: [(&str, &str, &str); 6] = [
        ("a truncated option", "truncated", "BASI"),
        ("an empty value", "emptyvalue", ""),
        ("an option padded with spaces", "padded", " BASIC "),
        (
            "a value of another enumerated member of the same template",
            "othersenum",
            "ON_DEMAND",
        ),
        ("the two options run together", "together", "BASICDETAILED"),
        ("a word that names no category", "verbose", "VERBOSE"),
    ];

    for (case, provider_name, requested) in rows {
        let (code, message) = refusal(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider": managed_instances_monitored(Some(requested)),
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, OUT_OF_SET),
            "case {case:?}: expected the member and its value set, got {code:?} with \
             {message:?}"
        );
        // The refused create stored nothing: the name it asked for describes no provider,
        // and the read reports it as one MISSING identifier instead.
        let body = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": [provider_name] }),
            case,
        )
        .await;
        assert!(
            list(&body, "capacityProviders", case).is_empty(),
            "case {case:?}: a refused create must leave no provider, got {body}"
        );
        let failures = list(&body, "failures", case);
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: expected the name reported as one failure, got {body}"
        );
        assert_eq!(
            failures[0]["reason"].as_str(),
            Some("MISSING"),
            "case {case:?}: expected the name reported MISSING, got {body}"
        );
    }
}

#[tokio::test]
async fn repeating_a_create_in_another_spelling_of_the_same_option_reports_the_stored_provider() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    // The option is stored in the modelled spelling, so a REPEAT that spells the same
    // option another way asks for the provider that is already stored: the `@idempotent`
    // create reports it unchanged rather than refusing the name as taken.
    let rows: [(&str, &str, &str, &str, &str); 3] = [
        (
            "the documented spelling after the set's own",
            "repeatbasic",
            "BASIC",
            "basic",
            "BASIC",
        ),
        (
            "the set's own spelling after the documented one",
            "repeatdetailed",
            "detailed",
            "DETAILED",
            "DETAILED",
        ),
        (
            "an alternating-case spelling after a capitalised one",
            "repeatmixed",
            "Basic",
            "bAsIc",
            "BASIC",
        ),
    ];

    for (case, provider_name, first, repeat, expected) in rows {
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider": managed_instances_monitored(Some(first)),
            }),
            case,
        )
        .await;
        let repeated = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider": managed_instances_monitored(Some(repeat)),
            }),
            case,
        )
        .await;
        assert_eq!(
            repeated["capacityProvider"]["capacityProviderArn"],
            created["capacityProvider"]["capacityProviderArn"],
            "case {case:?}: expected the repeat to report the stored provider, got {repeated}"
        );
        let stored = stored_monitoring(&router, provider_name, case).await;
        assert_eq!(
            stored.as_deref(),
            Some(expected),
            "case {case:?}: expected the stored option unchanged by the repeat, got {stored:?}"
        );
    }
}
