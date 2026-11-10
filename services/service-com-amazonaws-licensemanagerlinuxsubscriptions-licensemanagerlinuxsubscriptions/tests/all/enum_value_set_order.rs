// SPDX-License-Identifier: CC0-1.0

//! WHICH ORDER a refusal lists an enumerated type's admissible values in, over the real
//! wire path: every call goes through the router's `dispatch`, so the clause is observed
//! exactly as a client reads it.
//!
//! The rule, stated once in `services/common/src/validation_message.rs` and applied by
//! this service's one refusal formatter (`src/service/validation.rs`): a value outside an
//! enumerated type is refused with a clause naming the values that type admits, and those
//! values are listed in the order this front door walks its own table of them — a table
//! keyed on each value's own text (`service_common::validation_message::hashed_enum_order`)
//! — rather than in the order the model declares them.
//!
//! The order is derived from the values themselves, so it belongs to the enumerated TYPE:
//! one type is listed alike whichever member carries it, in whatever operation, at
//! whatever depth, and whatever the request's shape. That is what the rows below walk —
//! the enum, the member, the operation, the depth, the position in a list, and the form of
//! the rejected value — and what the two guards at the end state independently of them.

use serde_json::json;

use service_common::validation_message::{enum_value_bucket, hashed_enum_order};

use crate::harness::{call, harness, list_providers, register, update_settings, Reply};

/// `POST /subscription/ListLinuxSubscriptions` (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
/// `POST /subscription/ListLinuxSubscriptionInstances` (`OPERATIONS.md`).
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// A `SecretArn` inside the modelled `@pattern`, so a row about another member's enum is
/// refused for that enum alone (`smithy_query(query="operation",
/// operation="RegisterSubscriptionProvider")`).
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token";

/// Every enumerated type of this model, as the values it DECLARES, in declaration order.
///
/// Source: `smithy_query(query="shape", shape="<Enum>")` for each of
/// `Operator`, `LinuxSubscriptionsDiscovery`, `OrganizationIntegration`,
/// `SubscriptionProviderSource`, `Status` and `SubscriptionProviderStatus`. The first
/// three and the fourth are carried by a REQUEST member, so a refusal can list them; the
/// last two are reported only in a response, and are here because the rule is about the
/// TYPE and holds for a type a later model revision starts accepting.
const MODELLED_ENUMS: &[(&str, &[&str])] = &[
    ("Operator", &["Equal", "NotEqual", "Contains"]),
    ("LinuxSubscriptionsDiscovery", &["Enabled", "Disabled"]),
    ("OrganizationIntegration", &["Enabled", "Disabled"]),
    ("SubscriptionProviderSource", &["RedHat"]),
    (
        "Status",
        &["InProgress", "Completed", "Successful", "Failed"],
    ),
    (
        "SubscriptionProviderStatus",
        &["ACTIVE", "INVALID", "PENDING"],
    ),
];

/// The values `name` declares, read out of [`MODELLED_ENUMS`] so this suite holds each
/// value set in ONE place: a row names the enum, never a second copy of its values.
fn declared(name: &str) -> &'static [&'static str] {
    MODELLED_ENUMS
        .iter()
        .find(|(enumerated, _)| *enumerated == name)
        .map(|(_, values)| *values)
        .unwrap_or_else(|| panic!("case {name:?}: this model declares no such enum"))
}

/// One row: a refused request, the member its clause is reported at, the value that was
/// rejected, and the enum whose values the clause has to list.
struct Row {
    case: String,
    reply: Reply,
    member: String,
    rejected: String,
    declared: &'static [&'static str],
    /// Whether the violation sits on an ELEMENT of a list, which nests the requirement and
    /// quotes the value as the collection formed from the elements that failed.
    nested: bool,
}

/// The clause the row's refusal must carry, with the value set put through the SHARED
/// derivation rather than transcribed — so a formatter that grew a second copy of the
/// order and drifted from it fails here.
fn expected_message(row: &Row) -> String {
    let listed = hashed_enum_order(row.declared).join(", ");
    let requirement = format!("Member must satisfy enum value set: [{listed}]");
    let (subject, requirement) = if row.nested {
        (
            format!("[{}]", row.rejected),
            format!("Member must satisfy constraint: [{requirement}]"),
        )
    } else {
        (row.rejected.clone(), requirement)
    };
    format!(
        "1 validation error detected: Value '{subject}' at '{}' failed to satisfy constraint: {requirement}",
        row.member
    )
}

/// One `Filters` list whose element at `position` (1-based) carries `operator`, and whose
/// other elements are filters the list documents.
fn filters(name: &str, position: usize, operator: &str, length: usize) -> serde_json::Value {
    let elements: Vec<serde_json::Value> = (1..=length)
        .map(|index| {
            json!({
                "Name": name,
                "Values": ["anything"],
                "Operator": if index == position { operator } else { "Equal" },
            })
        })
        .collect();
    json!({ "Filters": elements })
}

/// The rule, over every request member of this model that carries an enumerated type.
#[tokio::test]
async fn an_enum_refusal_lists_the_values_in_the_order_the_front_door_states_them() {
    let h = harness();

    let mut rows: Vec<Row> = Vec::new();

    // The `Filter$Operator` of BOTH discovery lists: the same enum, two operations. The
    // rows walk the rejected value's form (a plain word, an operator spelled as the
    // documentation words it, a differently-cased member of the set, a value carrying
    // punctuation), the POSITION of the offending filter, and how many filters the
    // request carries — none of which may reach the listing.
    //
    // `Filter$Name` is a name each list documents
    // (`smithy_query(query="operation", operation="ListLinuxSubscriptions")` and the same
    // for `ListLinuxSubscriptionInstances`), so the enum is the one constraint the request
    // fails. The value stays inside the modelled `@length 1..20` of `Operator` for the
    // same reason.
    let operator_forms: &[(&str, &str)] = &[
        ("a word outside the set", "NotAnOperator"),
        ("the documented lowercase spelling", "equals"),
        ("a member of the set, recased", "notequal"),
        ("a value carrying punctuation", "Equal!"),
    ];
    for (list, operation, name) in [
        (SUBSCRIPTIONS, "ListLinuxSubscriptions", "Subscription"),
        (INSTANCES, "ListLinuxSubscriptionInstances", "Region"),
    ] {
        for (form, operator) in operator_forms {
            // (the offending element's 1-based position, how many filters are sent)
            for (position, length) in [(1, 1), (1, 3), (2, 2), (3, 3)] {
                rows.push(Row {
                    case: format!(
                        "{operation} with {form} at filter {position} of {length}"
                    ),
                    reply: call(&h, list, &filters(name, position, operator, length)).await,
                    member: format!("filters.{position}.member.operator"),
                    rejected: (*operator).to_string(),
                    declared: declared("Operator"),
                    nested: false,
                });
            }
        }
    }

    // The two `Enabled | Disabled` switches of the settings write: one at the top level of
    // the request, one inside a structure member, so the same value set is listed at two
    // depths.
    for (form, value) in [
        ("a switch outside the set", "On"),
        ("a member of the set, recased", "enabled"),
    ] {
        rows.push(Row {
            case: format!("UpdateServiceSettings with {form}"),
            reply: update_settings(
                &h,
                &json!({
                    "LinuxSubscriptionsDiscovery": value,
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": ["us-west-2"],
                        "OrganizationIntegration": "Disabled",
                    },
                    "AllowUpdate": true,
                }),
            )
            .await,
            member: "linuxSubscriptionsDiscovery".to_string(),
            rejected: value.to_string(),
            declared: declared("LinuxSubscriptionsDiscovery"),
            nested: false,
        });
        rows.push(Row {
            case: format!("UpdateServiceSettings with {form} for the organization switch"),
            reply: update_settings(
                &h,
                &json!({
                    "LinuxSubscriptionsDiscovery": "Enabled",
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": ["us-west-2"],
                        "OrganizationIntegration": value,
                    },
                    "AllowUpdate": true,
                }),
            )
            .await,
            member: "linuxSubscriptionsDiscoverySettings.organizationIntegration".to_string(),
            rejected: value.to_string(),
            declared: declared("OrganizationIntegration"),
            nested: false,
        });
    }

    // `SubscriptionProviderSource`: the write's own top-level member, and the same enum as
    // the ELEMENT type of the list's selection — where the requirement is nested and the
    // clause is reported at the collection, with the listing unchanged by either.
    for (form, value) in [
        ("a provider outside the set", "SUSE"),
        ("the member of the set, recased", "redhat"),
    ] {
        rows.push(Row {
            case: format!("RegisterSubscriptionProvider with {form}"),
            reply: register(
                &h,
                &json!({ "SubscriptionProviderSource": value, "SecretArn": SECRET_ARN }),
            )
            .await,
            member: "subscriptionProviderSource".to_string(),
            rejected: value.to_string(),
            declared: declared("SubscriptionProviderSource"),
            nested: false,
        });
        rows.push(Row {
            case: format!("ListRegisteredSubscriptionProviders with {form}"),
            reply: list_providers(&h, &json!({ "SubscriptionProviderSources": [value] })).await,
            member: "subscriptionProviderSources".to_string(),
            rejected: value.to_string(),
            declared: declared("SubscriptionProviderSource"),
            nested: true,
        });
    }

    for row in &rows {
        let case = &row.case;
        row.reply.error(case, 400, "ValidationException");
        let expected = expected_message(row);
        assert_eq!(
            row.reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            row.reply.message()
        );
    }

    // A row carrying a modelled value is accepted by the same members, so the rows above
    // are refused for the enum and not for something every request of that shape fails.
    let accepted = call(&h, SUBSCRIPTIONS, &filters("Subscription", 1, "Contains", 2)).await;
    accepted.ok("ListLinuxSubscriptions with a modelled operator");

    // The INDEPENDENT pin, on the wire: the rows above read their listing from the shared
    // derivation, so one row per discovery list states the clause as a literal — the
    // listing recorded for this refusal — which a derivation that changed consistently
    // with those rows would not satisfy.
    for (list, operation, name) in [
        (SUBSCRIPTIONS, "ListLinuxSubscriptions", "Subscription"),
        (INSTANCES, "ListLinuxSubscriptionInstances", "Region"),
    ] {
        let case = format!("{operation} with an operator outside the set");
        let reply = call(&h, list, &filters(name, 1, "NotAnOperator", 1)).await;
        reply.error(&case, 400, "ValidationException");
        let expected = "1 validation error detected: Value 'NotAnOperator' at \
                        'filters.1.member.operator' failed to satisfy constraint: Member must \
                        satisfy enum value set: [NotEqual, Equal, Contains]";
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

/// GUARD on the expectations above, which read the listing from the shared derivation: the
/// `Operator` set is listed `[NotEqual, Equal, Contains]`, which is the listing recorded
/// for this service's refusal of an unmodelled `Filter$Operator` — and the model declares
/// that set `[Equal, NotEqual, Contains]`, so the listing is NOT the declaration.
#[test]
fn the_operator_value_set_is_listed_as_the_recorded_refusal_lists_it() {
    const RECORDED: &[&str] = &["NotEqual", "Equal", "Contains"];

    // Whichever order the set is presented in — the model's, reversed, or rotated — the
    // listing is the one the service states, because it is derived from the values.
    let presentations: &[(&str, &[&str])] = &[
        ("the model's declaration order", declared("Operator")),
        ("reversed", &["Contains", "NotEqual", "Equal"]),
        ("rotated", &["NotEqual", "Contains", "Equal"]),
        ("the recorded listing itself", RECORDED),
    ];
    for (case, presented) in presentations {
        let listed = hashed_enum_order(presented);
        assert_eq!(
            listed, RECORDED,
            "case {case:?}: expected {RECORDED:?}, got {listed:?}"
        );
    }
    assert_ne!(
        hashed_enum_order(declared("Operator")),
        declared("Operator").to_vec(),
        "case the declaration order: the listing must not be the model's declaration, got \
         {:?}",
        declared("Operator")
    );
}

/// GUARD on the derivation itself, over EVERY enumerated type this model declares: the
/// listing holds exactly the values the type admits — none added, none dropped, none
/// repeated — and, for a type no two of whose values share a bucket of the table, it does
/// not depend on the order the values were presented in.
#[test]
fn every_modelled_value_set_is_listed_whole_and_independently_of_its_presentation() {
    for (name, declared) in MODELLED_ENUMS {
        let listed = hashed_enum_order(declared);
        let mut listed_sorted = listed.clone();
        listed_sorted.sort_unstable();
        let mut declared_sorted = declared.to_vec();
        declared_sorted.sort_unstable();
        assert_eq!(
            listed_sorted, declared_sorted,
            "case {name:?}: the listing must hold exactly the declared values, got {listed:?}"
        );

        // Two values in one bucket are read in the order they were pushed onto it, so the
        // claim below is made only for a type whose values sit in buckets of their own.
        let mut buckets: Vec<u32> = declared.iter().map(|value| enum_value_bucket(value)).collect();
        buckets.sort_unstable();
        let collides = buckets.windows(2).any(|pair| pair[0] == pair[1]);
        if collides {
            continue;
        }
        let reversed: Vec<&str> = declared.iter().rev().copied().collect();
        let from_reversed = hashed_enum_order(&reversed);
        assert_eq!(
            from_reversed, listed,
            "case {name:?}: the listing must not depend on the presentation, got {from_reversed:?} \
             from the reversed set and {listed:?} from the declared one"
        );
    }
}
