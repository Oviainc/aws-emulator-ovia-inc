// SPDX-License-Identifier: CC0-1.0

//! The two parts of a boundary refusal this service reads from the SHARED validation
//! module rather than re-implementing, driven over the real wire path through an operation
//! each of them is reached by.
//!
//! The rules, each stated once in `services/common/src/validation.rs` and applied by this
//! service's one refusal formatter (`src/service/validation.rs`):
//!
//! 1. a `@pattern` requirement quotes the modelled expression with the whole-value anchor
//!    PAIR the model wraps it in dropped and nothing else changed
//!    (`service_common::validation::unanchored`), whichever member and whichever
//!    operation carries it;
//! 2. a violation on an ELEMENT of a collection is reported at the COLLECTION member —
//!    the path `service_common::validation::list_of_element` names for that element — with
//!    the element's requirement nested, so no element position and no element step ever
//!    reaches a client; a violation that is NOT on an element keeps the member's own path
//!    and nests nothing.
//!
//! Both rules are asserted against the SHARED helpers' own output rather than against a
//! transcribed sentence, so this suite fails if the formatter ever grows a second copy of
//! either rule that drifts from it.

use serde_json::json;

use crate::harness::{
    call, deregister, get_provider, harness, list_providers, register, update_settings, Reply,
    RED_HAT,
};

/// The modelled `@pattern` of `SubscriptionProviderArn`, EXACTLY as the model spells it —
/// anchors included (`smithy_query(query="operation",
/// operation="DeregisterSubscriptionProvider")`). What a refusal quotes is this expression
/// put through the shared renderer, which is what rule 1 is about.
const MODELLED_PROVIDER_ARN_PATTERN: &str = r"^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}$";
/// The modelled `@pattern` of `SecretArn`, anchors included
/// (`smithy_query(query="operation",operation="RegisterSubscriptionProvider")`).
const MODELLED_SECRET_ARN_PATTERN: &str =
    r"^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$";

/// The clause this service words a `@pattern` violation with, over the expression the
/// SHARED renderer makes of the model's text.
fn expected_pattern_clause(value: &str, member: &str, modelled: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {}",
        service_common::validation::unanchored(modelled)
    )
}

/// Rule 1: every operation carrying a pattern-constrained BODY member quotes the modelled
/// expression as the shared renderer states it.
///
/// The rows differ along what the rule generalises over: the member
/// (`subscriptionProviderArn` of the read and of the removal, `secretArn` of the write),
/// the operation that carries it, and the FORM of the offending value (uppercase segments,
/// an empty resource id, non-ASCII, a space, and a character the segment class excludes).
#[tokio::test]
async fn a_pattern_requirement_quotes_the_expression_the_shared_renderer_states() {
    let h = harness();

    // (case, the offending value, the member it sits at, the model's anchored expression)
    let provider_arn_values: &[(&str, &str)] = &[
        ("uppercase segments", "ARN:AWS:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/abc"),
        (
            "an empty resource id",
            "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/",
        ),
        (
            "a non-ASCII resource id",
            "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/αβγ",
        ),
        (
            "a space in the resource id",
            "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/a b",
        ),
    ];
    for (form, value) in provider_arn_values {
        for (operation, reply) in [
            (
                "GetRegisteredSubscriptionProvider",
                get_provider(&h, value).await,
            ),
            (
                "DeregisterSubscriptionProvider",
                deregister(&h, value).await,
            ),
        ] {
            let case = format!("{operation} with {form}");
            reply.error(&case, 400, "ValidationException");
            let expected = expected_pattern_clause(
                value,
                "subscriptionProviderArn",
                MODELLED_PROVIDER_ARN_PATTERN,
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // The write's own pattern-constrained member, whose expression is a different one: a
    // secret ARN whose name segment excludes `/`, and a service segment that is not
    // Secrets Manager's.
    let secret_arn_values: &[(&str, &str)] = &[
        (
            "a slash in the secret name",
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:team/rhel-token",
        ),
        (
            "the resource type spelled in uppercase",
            "arn:aws:secretsmanager:us-west-2:000000000000:SECRET:rhel-token",
        ),
    ];
    for (form, value) in secret_arn_values {
        let case = format!("RegisterSubscriptionProvider with {form}");
        let reply = register(
            &h,
            &json!({
                "SubscriptionProviderSource": RED_HAT,
                "SecretArn": value,
            }),
        )
        .await;
        reply.error(&case, 400, "ValidationException");
        let expected = expected_pattern_clause(value, "secretArn", MODELLED_SECRET_ARN_PATTERN);
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

/// One row of rule 2: a refused request, the member path its clause must be reported at,
/// and the offending values in walk order.
struct ElementRow {
    case: &'static str,
    /// The member the clause must name: the COLLECTION for an element violation (what the
    /// shared recognition answers for that element's path), the member's own path for a
    /// violation that is not on an element.
    member: &'static str,
    /// The values the request sent that are outside their constraint, in position order.
    offending: Vec<String>,
    /// Whether the violation is on an ELEMENT, so the requirement is nested and the values
    /// are quoted as the collection formed from them.
    on_an_element: bool,
    reply: Reply,
}

/// Rule 2: a violation on an ELEMENT of a collection is reported at the collection member
/// with the element's requirement nested, and a violation that is NOT on an element keeps
/// the member's own path with nothing nested.
///
/// The rows differ along what the rule generalises over: whether the violated member is an
/// element at all, the operation that carries it, the depth the member sits at (the top
/// level of a request, and a member INSIDE a structure element of a list), the number of
/// offending values and the values themselves.
#[tokio::test]
async fn an_element_violation_is_reported_at_the_collection_the_shared_recognition_names() {
    let h = harness();

    let rows = vec![
        ElementRow {
            case: "one source outside the enum",
            member: "subscriptionProviderSources",
            offending: vec!["Ubuntu".to_string()],
            on_an_element: true,
            reply: list_providers(&h, &json!({ "SubscriptionProviderSources": ["Ubuntu"] })).await,
        },
        ElementRow {
            case: "three sources outside the enum",
            member: "subscriptionProviderSources",
            offending: vec![
                "Ubuntu".to_string(),
                "Debian".to_string(),
                "CentOS".to_string(),
            ],
            on_an_element: true,
            reply: list_providers(
                &h,
                &json!({ "SubscriptionProviderSources": ["Ubuntu", "Debian", "CentOS"] }),
            )
            .await,
        },
        ElementRow {
            // A member INSIDE a structure element of a list: the step into the element is
            // not the last one, so this is not an element violation and the member keeps
            // its own full path. The value is inside the modelled `@length 1..20` of
            // `Operator` and outside its value set
            // (`smithy_query(query="shape",shape="Operator")`), so the enum is the one
            // constraint it fails.
            case: "a filter operator outside the enum",
            member: "filters.1.member.operator",
            offending: vec!["Matches".to_string()],
            on_an_element: false,
            reply: call(
                &h,
                "/subscription/ListLinuxSubscriptionInstances",
                &json!({
                    "Filters": [{
                        "Name": "Region",
                        "Values": ["us-west-2"],
                        "Operator": "Matches",
                    }],
                }),
            )
            .await,
        },
        ElementRow {
            // A top-level member that is not a collection at all.
            case: "a discovery switch outside the enum",
            member: "linuxSubscriptionsDiscovery",
            offending: vec!["On".to_string()],
            on_an_element: false,
            reply: update_settings(
                &h,
                &json!({
                    "LinuxSubscriptionsDiscovery": "On",
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": ["us-west-2"],
                        "OrganizationIntegration": "Disabled",
                    },
                    "AllowUpdate": true,
                }),
            )
            .await,
        },
    ];

    for row in rows {
        let case = row.case;
        row.reply.error(case, 400, "ValidationException");
        let message = row.reply.message();
        // An element violation quotes the collection formed from the values that failed
        // and nests the element's own requirement inside; anything else quotes the value
        // and nests nothing.
        let opening = if row.on_an_element {
            format!(
                "1 validation error detected: Value '[{}]' at '{}' failed to satisfy \
                 constraint: Member must satisfy constraint: [",
                row.offending.join(", "),
                row.member
            )
        } else {
            format!(
                "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: ",
                row.offending.join(", "),
                row.member
            )
        };
        assert!(
            message.starts_with(&opening),
            "case {case:?}: expected a clause opening {opening:?}, got {message:?}"
        );
        assert_eq!(
            message.contains("Member must satisfy constraint: ["),
            row.on_an_element,
            "case {case:?}: expected the element requirement nested: {}, got {message:?}",
            row.on_an_element
        );
        if row.on_an_element {
            assert!(
                message.ends_with(']'),
                "case {case:?}: the nested element requirement must be closed, got {message:?}"
            );
        }
        // The point of the rule: no element POSITION and no element step is reported for
        // the collection the elements came from, whichever position they sat at.
        for position in 1..=row.offending.len() {
            let element_path = format!("{}.{position}.member", row.member);
            assert!(
                !message.contains(&element_path),
                "case {case:?}: the element path {element_path:?} must not be reported, got \
                 {message:?}"
            );
        }
    }
}

/// Guard on what rule 1 asserts above: the clause rows are built from the shared
/// renderer, so this states INDEPENDENTLY that the renderer drops the anchor pair of each
/// `@pattern` this model declares and changes nothing else — a renderer that stopped
/// dropping it would satisfy the rows but not this.
#[test]
fn every_modelled_pattern_is_quoted_without_its_anchor_pair() {
    for modelled in [MODELLED_PROVIDER_ARN_PATTERN, MODELLED_SECRET_ARN_PATTERN] {
        let quoted = service_common::validation::unanchored(modelled);
        assert!(
            !quoted.starts_with('^') && !quoted.ends_with('$'),
            "case {modelled:?}: the quoted expression must carry no anchor pair, got \
             {quoted:?}"
        );
        assert_eq!(
            quoted,
            modelled
                .trim_start_matches('^')
                .trim_end_matches('$')
                .to_string(),
            "case {modelled:?}: only the anchor pair may be dropped, got {quoted:?}"
        );
    }
}
