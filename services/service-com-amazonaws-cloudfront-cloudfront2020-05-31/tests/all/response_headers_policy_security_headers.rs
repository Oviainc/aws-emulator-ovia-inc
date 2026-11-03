// SPDX-License-Identifier: CC0-1.0

//! How a serialized response headers policy reports its `SecurityHeadersConfig`, end to
//! end over HTTP.
//!
//! The rule under test: in a serialized policy, EVERY member
//! `ResponseHeadersPolicySecurityHeadersConfig` declares occupies an element, in the order
//! the model declares them — a security header the configuration set carries the
//! directives it was configured with, and one it did not is the member's EMPTY element.
//! So a client reading the structure sees the same element set for every policy and can
//! tell "this header is not configured" from "this member was not serialized".
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization. The rule is
//! asserted on EVERY path that puts a policy on the wire: the create that mints it, both
//! reads of the whole policy and of its configuration alone, and the listing's embedded
//! summary — one serializer, so no two of them may disagree about one policy.
//!
//! The rows span what the rule generalises over: the subset of the six members a
//! configuration sets (none of them, each one alone, two together, all six, and the
//! published set that leaves exactly one out), the directives inside a member that is set
//! (both values of each boolean, the optional sub-members present and absent), and the
//! source of the policy (one this account created, and one AWS published). The member
//! catalogue the rows are written against is checked against the shape's own schema, so a
//! member the model adds fails here rather than reaching the wire unnoticed.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::RESPONSE_HEADERS_POLICY_SECURITY_HEADERS_CONFIG_SCHEMA;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MANAGED_RESPONSE_HEADERS_POLICIES;

use crate::distribution_lifecycle::{emulator, Answer, Node, API_VERSION};
use crate::response_headers_policy_config_read::get_policy_config;
use crate::response_headers_policy_lifecycle::{
    assert_empty_element, create_policy, get_policy, text,
};
use crate::response_headers_policy_refusal_message::list_policies;

// ---------------------------------------------------------------------------
// The members of the shape
// ---------------------------------------------------------------------------

/// Every member `ResponseHeadersPolicySecurityHeadersConfig` declares, in the order the
/// model declares them
/// (`smithy_query(query="shape",shape="ResponseHeadersPolicySecurityHeadersConfig")`).
///
/// [`the_catalogue_is_the_shapes_own_member_list`] checks this list against the shape's
/// generated schema, so it cannot drift from the model.
const MEMBERS: [&str; 6] = [
    "XSSProtection",
    "FrameOptions",
    "ReferrerPolicy",
    "ContentSecurityPolicy",
    "ContentTypeOptions",
    "StrictTransportSecurity",
];

/// One security header a configuration sets: the member it occupies, the directives a
/// request states inside that member, and the `(child element, text)` pairs the answer
/// must report back for it.
#[derive(Debug, Clone)]
struct Header {
    member: &'static str,
    body: String,
    expected: Vec<(&'static str, String)>,
}

/// `XSSProtection` with both of its `@required` directives and, optionally, each of its
/// two optional ones.
fn xss(overridden: bool, protection: bool, mode_block: Option<bool>, report: Option<&str>) -> Header {
    let mut body = format!("<Override>{overridden}</Override><Protection>{protection}</Protection>");
    let mut expected = vec![
        ("Override", overridden.to_string()),
        ("Protection", protection.to_string()),
    ];
    if let Some(mode_block) = mode_block {
        body.push_str(&format!("<ModeBlock>{mode_block}</ModeBlock>"));
        expected.push(("ModeBlock", mode_block.to_string()));
    }
    if let Some(report) = report {
        body.push_str(&format!("<ReportUri>{report}</ReportUri>"));
        expected.push(("ReportUri", report.to_string()));
    }
    Header {
        member: "XSSProtection",
        body,
        expected,
    }
}

/// `FrameOptions`, whose `FrameOption` is a value of the model's `FrameOptionsList`.
fn frame_options(overridden: bool, option: &str) -> Header {
    Header {
        member: "FrameOptions",
        body: format!(
            "<Override>{overridden}</Override><FrameOption>{option}</FrameOption>"
        ),
        expected: vec![
            ("Override", overridden.to_string()),
            ("FrameOption", option.to_string()),
        ],
    }
}

/// `ReferrerPolicy`, whose `ReferrerPolicy` is a value of the model's `ReferrerPolicyList`.
fn referrer_policy(overridden: bool, policy: &str) -> Header {
    Header {
        member: "ReferrerPolicy",
        body: format!(
            "<Override>{overridden}</Override><ReferrerPolicy>{policy}</ReferrerPolicy>"
        ),
        expected: vec![
            ("Override", overridden.to_string()),
            ("ReferrerPolicy", policy.to_string()),
        ],
    }
}

/// `ContentSecurityPolicy` and the directives it carries.
fn content_security_policy(overridden: bool, directives: &str) -> Header {
    Header {
        member: "ContentSecurityPolicy",
        body: format!(
            "<Override>{overridden}</Override>\
             <ContentSecurityPolicy>{directives}</ContentSecurityPolicy>"
        ),
        expected: vec![
            ("Override", overridden.to_string()),
            ("ContentSecurityPolicy", directives.to_string()),
        ],
    }
}

/// `ContentTypeOptions`, whose only member is its `Override`.
fn content_type_options(overridden: bool) -> Header {
    Header {
        member: "ContentTypeOptions",
        body: format!("<Override>{overridden}</Override>"),
        expected: vec![("Override", overridden.to_string())],
    }
}

/// `StrictTransportSecurity` with its two `@required` directives and, optionally, each of
/// its two optional ones.
fn strict_transport_security(
    overridden: bool,
    include_subdomains: Option<bool>,
    preload: Option<bool>,
    max_age: i32,
) -> Header {
    let mut body = format!("<Override>{overridden}</Override>");
    let mut expected = vec![("Override", overridden.to_string())];
    if let Some(include) = include_subdomains {
        body.push_str(&format!("<IncludeSubdomains>{include}</IncludeSubdomains>"));
        expected.push(("IncludeSubdomains", include.to_string()));
    }
    if let Some(preload) = preload {
        body.push_str(&format!("<Preload>{preload}</Preload>"));
        expected.push(("Preload", preload.to_string()));
    }
    body.push_str(&format!(
        "<AccessControlMaxAgeSec>{max_age}</AccessControlMaxAgeSec>"
    ));
    expected.push(("AccessControlMaxAgeSec", max_age.to_string()));
    Header {
        member: "StrictTransportSecurity",
        body,
        expected,
    }
}

/// One row: the security headers a configuration sets, and nothing else about the policy.
#[derive(Debug, Clone)]
struct Case {
    case: &'static str,
    configured: Vec<Header>,
}

/// The `ResponseHeadersPolicyConfig` document a row sends, in the model's member order.
fn request_xml(case: &Case, name: &str) -> String {
    let mut out = format!(
        "<ResponseHeadersPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><SecurityHeadersConfig>"
    );
    // In the model's own member order, so the request document is one a client writes.
    for member in MEMBERS {
        if let Some(header) = case.configured.iter().find(|set| set.member == member) {
            out.push_str(&format!("<{member}>{}</{member}>", header.body));
        }
    }
    out.push_str("</SecurityHeadersConfig></ResponseHeadersPolicyConfig>");
    out
}

/// The rows the rule is asserted over: every subset shape the six members admit that the
/// rule generalises over, and the directive values inside the members that are set.
fn table() -> Vec<Case> {
    vec![
        Case {
            case: "a security headers configuration that sets no header at all",
            configured: Vec::new(),
        },
        Case {
            case: "XSS protection alone, with both optional directives",
            configured: vec![xss(true, true, Some(true), None)],
        },
        Case {
            case: "frame options alone",
            configured: vec![frame_options(false, "DENY")],
        },
        Case {
            case: "a referrer policy alone",
            configured: vec![referrer_policy(true, "no-referrer")],
        },
        Case {
            case: "a content security policy alone",
            configured: vec![content_security_policy(false, "default-src 'self'")],
        },
        Case {
            case: "content type options alone",
            configured: vec![content_type_options(true)],
        },
        Case {
            // The shape the recorded create takes: one member set, five unset.
            case: "strict transport security alone",
            configured: vec![strict_transport_security(true, None, None, 63_072_000)],
        },
        Case {
            case: "frame options and content type options together",
            configured: vec![frame_options(true, "SAMEORIGIN"), content_type_options(false)],
        },
        Case {
            case: "every one of the six headers set",
            configured: vec![
                xss(false, false, Some(false), Some("https://report.example/xss")),
                frame_options(true, "SAMEORIGIN"),
                referrer_policy(false, "strict-origin-when-cross-origin"),
                content_security_policy(true, "default-src 'none'; img-src 'self'"),
                content_type_options(true),
                strict_transport_security(false, Some(true), Some(false), 31_536_000),
            ],
        },
        Case {
            // The set the published policies carry: every header but the content security
            // policy, which AWS leaves to the account's own policy.
            case: "five headers set, the content security policy left out",
            configured: vec![
                xss(false, true, Some(true), None),
                frame_options(false, "SAMEORIGIN"),
                referrer_policy(false, "strict-origin-when-cross-origin"),
                content_type_options(false),
                strict_transport_security(false, Some(true), Some(false), 31_536_000),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading one answer
// ---------------------------------------------------------------------------

/// Assert the `SecurityHeadersConfig` under `config` reports every member of the shape:
/// the ones `configured` set with their directives, the rest as empty elements, in the
/// order the model declares them.
fn assert_security_headers(config: &Node, configured: &[Header], case: &str, context: &str) {
    let security = config.at("SecurityHeadersConfig").unwrap_or_else(|| {
        panic!("case {case} ({context}): the answer carries no SecurityHeadersConfig element")
    });

    let reported: Vec<&str> = security
        .children
        .iter()
        .map(|child| child.name())
        .collect();
    assert_eq!(
        reported,
        MEMBERS.to_vec(),
        "case {case} ({context}): expected every member of the shape in the model's order \
         {MEMBERS:?}, got {reported:?}"
    );

    for member in MEMBERS {
        match configured.iter().find(|header| header.member == member) {
            Some(header) => {
                for (child, expected) in &header.expected {
                    let actual = text(
                        security,
                        &format!("{member}/{child}"),
                        case,
                        context,
                        "the configured header",
                    );
                    assert_eq!(
                        &actual, expected,
                        "case {case} ({context}): expected {member}/{child} {expected:?}, got \
                         {actual:?}"
                    );
                }
                let directives: Vec<&str> = security
                    .at(member)
                    .map(|element| {
                        element.children.iter().map(|child| child.name()).collect()
                    })
                    .unwrap_or_default();
                let expected: Vec<&str> =
                    header.expected.iter().map(|(child, _)| *child).collect();
                assert_eq!(
                    directives, expected,
                    "case {case} ({context}): expected {member} to carry exactly the configured \
                     directives {expected:?}, got {directives:?}"
                );
            }
            None => assert_empty_element(security, member, case, context),
        }
    }
}

/// The `ResponseHeadersPolicyConfig` the summary of the policy `id` carries in a listing,
/// or a panic naming the case.
fn listed_config<'a>(listing: &'a Answer, id: &str, case: &str) -> &'a Node {
    listing
        .root()
        .at("Items")
        .map(|items| items.children_named("ResponseHeadersPolicySummary"))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| {
            summary.text_at("ResponseHeadersPolicy/Id") == Some(id)
        })
        .and_then(|summary| summary.at("ResponseHeadersPolicy/ResponseHeadersPolicyConfig"))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the listing carries no summary for {id}: {}",
                listing.raw
            )
        })
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// The catalogue the rows are written against is the shape's OWN member list, in the
/// shape's own order: a member the model adds (or renames) fails here, rather than being
/// serialized as an unset header by the projection's default.
#[test]
fn the_catalogue_is_the_shapes_own_member_list() {
    let declared: Vec<&str> = RESPONSE_HEADERS_POLICY_SECURITY_HEADERS_CONFIG_SCHEMA
        .members()
        .iter()
        .filter_map(|member| member.member_name())
        .collect();
    assert_eq!(
        declared,
        MEMBERS.to_vec(),
        "expected the shape's members to be {MEMBERS:?}, got {declared:?}"
    );
}

/// Every path that serializes a policy this account created reports the whole
/// `SecurityHeadersConfig`: the headers the create configured, and the empty element for
/// each one it did not.
#[tokio::test]
async fn every_member_of_the_security_headers_configuration_is_serialized() {
    let router = emulator();

    for (row, case) in table().into_iter().enumerate() {
        let name = format!("security-headers-{row}");
        let created = create_policy(&router, &request_xml(&case, &name)).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from the create, got {} with {}",
            case.case, created.status, created.raw
        );
        let config = created
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the create carried no ResponseHeadersPolicyConfig: {}",
                    case.case, created.raw
                )
            });
        assert_security_headers(config, &case.configured, case.case, "the create");

        let id = text(created.root(), "Id", case.case, "the create", &created.raw);

        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetResponseHeadersPolicy, got {} with {}",
            case.case, read.status, read.raw
        );
        let config = read
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the read carried no ResponseHeadersPolicyConfig: {}",
                    case.case, read.raw
                )
            });
        assert_security_headers(config, &case.configured, case.case, "the policy read");

        let config_read = get_policy_config(&router, &id).await;
        assert_eq!(
            config_read.status, 200,
            "case {}: expected HTTP 200 from GetResponseHeadersPolicyConfig, got {} with {}",
            case.case, config_read.status, config_read.raw
        );
        // `ResponseHeadersPolicyConfig` is that output's `@httpPayload`, so the
        // configuration IS the document's root element.
        assert_security_headers(
            config_read.root(),
            &case.configured,
            case.case,
            "the config read",
        );

        let listing = list_policies(&router, "?Type=custom&MaxItems=100").await;
        assert_eq!(
            listing.status, 200,
            "case {}: expected HTTP 200 from ListResponseHeadersPolicies, got {} with {}",
            case.case, listing.status, listing.raw
        );
        assert_security_headers(
            listed_config(&listing, &id, case.case),
            &case.configured,
            case.case,
            "the listed summary",
        );
    }
}

/// A policy AWS published is serialized by the same rule: the published set leaves the
/// content security policy to the account, so that member is the empty element while the
/// other five carry the published directives — and a published policy that configures no
/// security headers at all carries no `SecurityHeadersConfig` element, because the rule is
/// about the MEMBERS of that structure and not about inventing the structure itself.
#[tokio::test]
async fn a_published_policy_reports_the_whole_structure_too() {
    let router = emulator();

    for managed in MANAGED_RESPONSE_HEADERS_POLICIES {
        let case = managed.name;
        let read = get_policy(&router, managed.id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the read of a published policy, got {} with {}",
            read.status, read.raw
        );
        let config = read
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the read carried no ResponseHeadersPolicyConfig: {}",
                    read.raw
                )
            });

        if !managed.security_headers {
            assert!(
                config.at("SecurityHeadersConfig").is_none(),
                "case {case}: a published policy that configures no security headers carries no \
                 SecurityHeadersConfig, got {:?}",
                config.at("SecurityHeadersConfig")
            );
            continue;
        }

        let security = config.at("SecurityHeadersConfig").unwrap_or_else(|| {
            panic!("case {case}: the published policy carries no SecurityHeadersConfig: {}", read.raw)
        });
        let reported: Vec<&str> = security
            .children
            .iter()
            .map(|child| child.name())
            .collect();
        assert_eq!(
            reported,
            MEMBERS.to_vec(),
            "case {case}: expected every member of the shape in the model's order {MEMBERS:?}, \
             got {reported:?}"
        );
        assert_empty_element(
            security,
            "ContentSecurityPolicy",
            case,
            "the published policy",
        );
        for member in ["XSSProtection", "FrameOptions", "ReferrerPolicy", "ContentTypeOptions", "StrictTransportSecurity"] {
            let element = security.at(member).unwrap_or_else(|| {
                panic!("case {case}: the published policy carries no {member}: {}", read.raw)
            });
            assert!(
                !element.children.is_empty(),
                "case {case}: expected the published {member} to carry its directives, got \
                 {element:?}"
            );
        }
    }
}
