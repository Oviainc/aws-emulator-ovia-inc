// SPDX-License-Identifier: CC0-1.0

//! How `ActivateType` answers the ARN a request names a PUBLIC extension by, end to end over
//! the awsQuery wire.
//!
//! RULE under test: **CloudFormation's registry resolves the extension from the ARN itself —
//! reading its kind, its publisher and its NAME out of `type/<kind>/<publisher>/<Type-Name>`
//! — so an ARN it can read no extension type name out of resolves no extension and is refused
//! with the registry's own modelled error, `CFNRegistryException` (HTTP 400), carrying
//! "TypeName is invalid". The refusal is the registry's whichever side of the modelled
//! `ThirdPartyTypeArn` pattern the value falls on: that pattern is narrower than the ARNs
//! CloudFormation parses, so it must not pre-empt the registry's own answer with the generic
//! request-validation code a client branches on differently.**
//!
//! What a type name IS comes from the model: `@length(10..204)` and
//! `@pattern("^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$")`
//! (`smithy_query(query="shape", shape="TypeName")`), and the two errors the operation models
//! are `CFNRegistryException` (400) and `TypeNotFoundException` (404)
//! (`smithy_query(query="operation", operation="ActivateType")`).
//!
//! The rows differ along what the rule generalises over: each kind the modelled
//! `ThirdPartyType` admits, a name carrying the `::MODULE` suffix, and the ways an ARN can
//! carry no type name — a name segment that is a version number, one with too few segments,
//! one with a segment below the modelled minimum, one past the modelled maximum, an ARN whose
//! name position holds a version id because the segments ahead of it are not the publisher
//! form (the ARN shape that carries the name one segment to the left), and a value naming no
//! `type/` path at all. Each refusal is read back
//! through `DescribeType`, so a refusal is shown to have written nothing.
//!
//! `third_party_extensions.rs` is the suite for what an accepted activation WRITES; this one
//! is which ARNs the registry accepts at all.

use crate::stack_lifecycle::{call, member, router, xml_text, ACCOUNT_ID, REGION};
use service_com_amazonaws_cloudformation_cloudformation::state::registry;

/// A publisher id inside the modelled `PublisherId` pattern `^[0-9a-zA-Z]{12,40}$`, which is
/// also the segment the `ThirdPartyTypeArn` pattern expects in an ARN's third position.
const PUBLISHER_ID: &str = "abcdef0123456789abcd";

/// The execution role an activation of a public RESOURCE extension must name, inside the
/// modelled `RoleARN2` pattern `^arn:.+:iam::[0-9]{12}:role/.+$`.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/extension-execution";

/// An ARN naming a public extension: `type/<kind>/<publisher>/<name segment>`, with the name
/// segment written exactly as the row gives it.
fn public_type_arn(kind: &str, publisher: &str, name_segment: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}::type/{}/{publisher}/{name_segment}",
        kind.to_ascii_lowercase()
    )
}

/// The ARN this account addresses an activation by.
fn activated_arn(kind: &str, type_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/{}/{}",
        kind.to_ascii_lowercase(),
        type_name.replace("::", "-")
    )
}

/// A name segment of `segments` dash-joined parts, each `letters` characters long: how a row
/// reaches the bounds the modelled pattern states without spelling a 200-character literal.
fn name_segment(segments: usize, letters: usize) -> String {
    let part = "A".repeat(letters);
    vec![part; segments].join("-")
}

/// One row: the ARN a request names the public extension by, and what must come back.
struct ArnCase {
    case: &'static str,
    kind: &'static str,
    /// The ARN's publisher segment.
    publisher: String,
    /// The ARN's name segment, in the dashed spelling an ARN carries.
    name_segment: String,
    /// The extension name the activation is held under, where the ARN names one.
    activates: Option<String>,
    /// WHICH of the registry's two sentences refuses an ARN that names no activatable
    /// extension: the name is not one the registry resolves, or the ARN pins a public version
    /// number past a name that is. Ignored on a row the ARN activates.
    refused_with: &'static str,
}

fn arn_cases() -> Vec<ArnCase> {
    vec![
        ArnCase {
            case: "a resource extension named by a well-formed ARN",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Store-Bucket".to_string(),
            activates: Some("Example::Store::Bucket".to_string()),
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a module, whose name carries the MODULE suffix the pattern admits",
            kind: registry::registry_type::MODULE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Network-Subnets-MODULE".to_string(),
            activates: Some("Example::Network::Subnets::MODULE".to_string()),
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a Hook",
            kind: registry::registry_type::HOOK,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Guard-Hook".to_string(),
            activates: Some("Example::Guard::Hook".to_string()),
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment at the modelled minimum segment length",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: name_segment(3, 2),
            activates: Some(name_segment(3, 2).replace('-', "::")),
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment at the modelled maximum segment length",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: name_segment(3, 64),
            activates: Some(name_segment(3, 64).replace('-', "::")),
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            // The ARN's segments are read by their POSITION: the segment after the kind is
            // the PUBLISHER where a further segment follows it, and the extension's own NAME
            // where none does — so an ARN of this shape carries `00000001` where the name
            // belongs, which is no extension type name, and resolves no extension at all.
            // (An ARN that names no publisher is the form an extension Amazon Web Services
            // published carries, which is why that form keeps resolving its own name.)
            case: "a version id past a name, under a publisher segment the modelled pattern \
                   does not admit",
            kind: registry::registry_type::RESOURCE,
            publisher: "SomeVendor-Thing-Thing".to_string(),
            name_segment: "00000001".to_string(),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment that is a version number, under a well-formed publisher",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "00000001".to_string(),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name carrying only two segments",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Store".to_string(),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name carrying four segments that are not the MODULE suffix",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Store-Bucket-Extra".to_string(),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment one character below the modelled minimum",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: name_segment(3, 1),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment one character past the modelled maximum",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: name_segment(3, 65),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
        ArnCase {
            case: "a name segment carrying a character the modelled pattern does not admit",
            kind: registry::registry_type::RESOURCE,
            publisher: PUBLISHER_ID.to_string(),
            name_segment: "Example-Store-Buck_t".to_string(),
            activates: None,
            refused_with: registry::INVALID_TYPE_NAME,
        },
    ]
}

#[tokio::test]
async fn an_arn_naming_no_extension_type_name_is_the_registrys_own_refusal() {
    for row in arn_cases() {
        // A fresh account per row, so each activation is the first one its registry saw.
        let router = router();
        let arn = public_type_arn(row.kind, &row.publisher, &row.name_segment);
        let (status_code, body) = call(
            &router,
            "ActivateType",
            &[
                member("PublicTypeArn", &arn),
                // Named on every row: a public RESOURCE extension's activation needs one,
                // and no other kind is refused for carrying it.
                member("ExecutionRoleArn", EXECUTION_ROLE_ARN),
            ],
        )
        .await;

        match &row.activates {
            Some(type_name) => {
                let expected = activated_arn(row.kind, type_name);
                assert_eq!(
                    status_code, 200,
                    "case {:?}: expected the activation to succeed, got {status_code} {body}",
                    row.case
                );
                assert_eq!(
                    xml_text(&body, "Arn").as_deref(),
                    Some(expected.as_str()),
                    "case {:?}: expected the activation's own ARN, got {body}",
                    row.case
                );
                // Round trip: the activation is readable at the ARN it answered, under the
                // name the public ARN carried.
                let (status_code, described) =
                    call(&router, "DescribeType", &[member("Arn", &expected)]).await;
                assert_eq!(
                    status_code, 200,
                    "case {:?}: expected the activation readable, got {status_code} \
                     {described}",
                    row.case
                );
                assert_eq!(
                    xml_text(&described, "TypeName").as_deref(),
                    Some(type_name.as_str()),
                    "case {:?}: expected it held under the name the ARN carried, got \
                     {described}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    status_code, 400,
                    "case {:?}: expected the registry to refuse the ARN, got {status_code} \
                     {body}",
                    row.case
                );
                assert_eq!(
                    (
                        xml_text(&body, "Code").unwrap_or_default(),
                        xml_text(&body, "Message").unwrap_or_default()
                    ),
                    (
                        "CFNRegistryException".to_string(),
                        row.refused_with.to_string()
                    ),
                    "case {:?}: expected the registry's own modelled refusal, got {body}",
                    row.case
                );
                // Nothing was written: the name the ARN carried is in no account's registry,
                // whatever that name was.
                let unwritten = activated_arn(row.kind, &row.name_segment.replace('-', "::"));
                let (status_code, described) =
                    call(&router, "DescribeType", &[member("Arn", &unwritten)]).await;
                assert_eq!(
                    status_code, 404,
                    "case {:?}: expected the refused activation to write nothing, got \
                     {status_code} {described}",
                    row.case
                );
            }
        }
    }
}

/// A value that names no `type/` path at all carries no extension to resolve either, so it
/// gets the same registry refusal — and a request naming no public extension at all is still
/// the registry's refusal rather than a request-validation one.
#[tokio::test]
async fn a_value_naming_no_extension_at_all_is_refused_by_the_registry_too() {
    for (case, arn) in [
        (
            "a cloudformation ARN of another resource kind",
            format!("arn:aws:cloudformation:{REGION}::stack/not-a-type/abcdef0123456789abcd"),
        ),
        (
            "an ARN whose type path carries one segment only",
            format!("arn:aws:cloudformation:{REGION}::type/resource"),
        ),
        (
            "an ARN whose type path carries an empty name segment",
            format!("arn:aws:cloudformation:{REGION}::type/resource/{PUBLISHER_ID}/"),
        ),
    ] {
        let router = router();
        let (status_code, body) = call(
            &router,
            "ActivateType",
            &[
                member("PublicTypeArn", &arn),
                member("ExecutionRoleArn", EXECUTION_ROLE_ARN),
            ],
        )
        .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the registry to refuse the ARN, got {status_code} {body}"
        );
        assert_eq!(
            (
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default()
            ),
            (
                "CFNRegistryException".to_string(),
                registry::INVALID_TYPE_NAME.to_string()
            ),
            "case {case:?}: expected the registry's own modelled refusal, got {body}"
        );
    }
}
