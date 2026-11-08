// SPDX-License-Identifier: CC0-1.0

//! The rule that binds a cache behaviour's LAMBDA@EDGE ASSOCIATIONS to a PUBLISHED
//! FUNCTION VERSION, end to end over HTTP.
//!
//! `LambdaFunctionAssociation$LambdaFunctionARN` states it: "You must specify the ARN of a
//! function version; you can't specify an alias or $LATEST." A version is addressed by the
//! function's own ARN qualified with the number it was given when it was published, so a
//! configuration whose association names anything else — a value that is no Lambda function
//! ARN, the unqualified function ARN, the `$LATEST` of a function nobody published, or an
//! alias name — is refused with `InvalidLambdaFunctionAssociation` (HTTP 400) and NOTHING is
//! created or changed. The refusal states the version requirement and then names the
//! refused entry's `LambdaFunctionARN`, which the rows assert on
//! (`lambda_edge_association_message` spans that echoed value on its own).
//!
//! What the rows span: an association qualified with a one-digit and a many-digit version
//! number; the unqualified ARN; `$LATEST`; an alias qualifier, including one spelled with
//! digits in it; a version written `0` and one written with a leading zero (no published
//! version is numbered either way); an empty qualifier; a non-Lambda ARN and a value that is
//! no ARN at all; another account and another function-name form; two associations where
//! only the second one is bad; the association carried by the DEFAULT cache behaviour and by
//! an ORDERED one; and every path that stores a configuration — the two creates and the
//! update.
//!
//! The Region a Lambda@Edge function has to live in is a rule of its own, which this change
//! does not implement, so every row here names a function in US East (N. Virginia) and no
//! row states an expectation about another Region.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, list_distributions,
    update_distribution, Answer, API_VERSION,
};

/// The fixed part of this refusal's message, spelled out here rather than imported from
/// the implementation: it is CloudFront's own text for this error class, so a constant
/// edited to some other wording fails these rows. The refused association's
/// `LambdaFunctionARN` follows it, which is what [`refusal_message`] builds.
pub(crate) const REFUSAL_PREFIX: &str = concat!(
    "The function ARN must reference a specific function version. ",
    "(The ARN must end with the version number.) ARN: "
);

/// The whole message the refusal of an association naming `arn` has to carry.
pub(crate) fn refusal_message(arn: &str) -> String {
    format!("{REFUSAL_PREFIX}{arn}")
}

/// One Lambda@Edge association a row submits: the ARN it names and the event type it fires
/// on (one of `EventType`'s four values).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Association {
    pub(crate) arn: &'static str,
    pub(crate) event_type: &'static str,
}

impl Association {
    /// The element as XML, in the member order the shape declares.
    fn xml(&self) -> String {
        format!(
            "<LambdaFunctionAssociation>\
             <LambdaFunctionARN>{}</LambdaFunctionARN>\
             <EventType>{}</EventType>\
             <IncludeBody>false</IncludeBody>\
             </LambdaFunctionAssociation>",
            self.arn, self.event_type
        )
    }
}

/// The `LambdaFunctionAssociations` wrapper for `associations`, stating its own length as
/// every CloudFront list wrapper does.
fn associations_xml(associations: &[Association]) -> String {
    let mut out = format!(
        "<LambdaFunctionAssociations><Quantity>{}</Quantity>",
        associations.len()
    );
    if !associations.is_empty() {
        out.push_str("<Items>");
        for association in associations {
            out.push_str(&association.xml());
        }
        out.push_str("</Items>");
    }
    out.push_str("</LambdaFunctionAssociations>");
    out
}

/// Which cache behaviour of a configuration carries the associations under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    /// `DefaultCacheBehavior` — the one every distribution has.
    Default,
    /// One entry of `CacheBehaviors` — the ordered behaviours, which declare the same
    /// member.
    Ordered,
}

/// The `DistributionConfig` body for `reference`, carrying `associations` on `behavior`.
///
/// Members are written in the order the shapes declare them (`DefaultCacheBehavior`:
/// `TargetOriginId`, `ViewerProtocolPolicy`, `LambdaFunctionAssociations`; `CacheBehavior`
/// prefixes that with `PathPattern`).
pub(crate) fn config_xml(
    reference: &str,
    behavior: Behavior,
    associations: &[Association],
) -> String {
    match behavior {
        Behavior::Default => config_xml_per_behavior(reference, Some(associations), None),
        Behavior::Ordered => config_xml_per_behavior(reference, None, Some(associations)),
    }
}

/// The `DistributionConfig` body for `reference` with the associations each cache behaviour
/// carries stated separately: `None` leaves that behaviour without the member at all (and,
/// for the ordered one, without any `CacheBehaviors` at all), while `Some(&[])` states the
/// empty wrapper.
///
/// The ONE body these suites submit, so a row that puts associations on BOTH behaviours
/// states the same configuration as a row that puts them on one.
pub(crate) fn config_xml_per_behavior(
    reference: &str,
    default: Option<&[Association]>,
    ordered: Option<&[Association]>,
) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig></Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>redirect-to-https</ViewerProtocolPolicy>"
    );
    if let Some(associations) = default {
        out.push_str(&associations_xml(associations));
    }
    out.push_str("</DefaultCacheBehavior>");
    if let Some(associations) = ordered {
        out.push_str(
            "<CacheBehaviors><Quantity>1</Quantity><Items>\
             <CacheBehavior><PathPattern>/images/*</PathPattern>\
             <TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>",
        );
        out.push_str(&associations_xml(associations));
        out.push_str("</CacheBehavior></Items></CacheBehaviors>");
    }
    out.push_str(&format!("<Comment>for {reference}</Comment>"));
    out.push_str(
        "<PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled></DistributionConfig>",
    );
    out
}

/// `POST /2020-05-31/distribution?WithTags`, so the rule is asserted on the second create
/// path too — both run the one create.
pub(crate) async fn create_distribution_with_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    config: &str,
) -> Answer {
    let body = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{config}<Tags/>\
         </DistributionConfigWithTags>"
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution?WithTags"),
        Some(&body),
        None,
    )
    .await
}

/// One row: the associations a configuration states, and whether CloudFront stores it.
struct Case {
    case: &'static str,
    associations: &'static [Association],
    /// `true` when every association names a published function version.
    accepted: bool,
    /// For a refused row, the ARN the refusal has to name: the first association of the
    /// configuration that breaks the rule. Empty on an accepted row.
    refused_arn: &'static str,
}

/// A function version qualifier: the number Lambda assigns when a version is published.
const VERSION_ONE: &str = "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1";

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no-association",
            associations: &[],
            accepted: true,
            refused_arn: "",
        },
        Case {
            case: "version-1",
            associations: &[Association {
                arn: VERSION_ONE,
                event_type: "viewer-request",
            }],
            accepted: true,
            refused_arn: "",
        },
        Case {
            // A version number is however many digits the function has versions.
            case: "version-many-digits",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1042",
                event_type: "origin-response",
            }],
            accepted: true,
            refused_arn: "",
        },
        Case {
            // Another account and a function name spelled with the other characters a
            // function name admits: what this rule constrains is the QUALIFIER, so a
            // version ARN is admitted whatever the function is called or who owns it.
            // (Lambda@Edge additionally requires the function to live in US East (N.
            // Virginia); that is a separate rule, which this suite therefore states no
            // expectation about — every row here stays in that Region.)
            case: "version-other-account-and-name-form",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:210987654321:function:edge.greeter_2:7",
                event_type: "viewer-response",
            }],
            accepted: true,
            refused_arn: "",
        },
        Case {
            // Four associations, one per event type, each on a published version.
            case: "every-event-type-on-a-version",
            associations: &[
                Association {
                    arn: VERSION_ONE,
                    event_type: "viewer-request",
                },
                Association {
                    arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:2",
                    event_type: "origin-request",
                },
                Association {
                    arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:3",
                    event_type: "origin-response",
                },
                Association {
                    arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:4",
                    event_type: "viewer-response",
                },
            ],
            accepted: true,
            refused_arn: "",
        },
        Case {
            // The function ARN with no qualifier at all: it addresses the function, not one
            // of its versions.
            case: "unqualified-function-arn",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter",
        },
        Case {
            case: "latest-qualifier",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:$LATEST",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:$LATEST",
        },
        Case {
            case: "alias-qualifier",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:live",
                event_type: "origin-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:live",
        },
        Case {
            // An alias name may carry digits; it is still a name, not a version number.
            case: "alias-qualifier-with-digits",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:v2",
                event_type: "origin-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:v2",
        },
        Case {
            // Published versions count from 1, so no function has a version 0.
            case: "version-zero",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:0",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:0",
        },
        Case {
            case: "version-leading-zero",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:01",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:01",
        },
        Case {
            case: "empty-qualifier",
            associations: &[Association {
                arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:",
        },
        Case {
            // A versioned ARN of another service is no Lambda function version.
            case: "not-a-lambda-arn",
            associations: &[Association {
                arn: "arn:aws:states:us-east-1:000000000000:stateMachine:edge-greeter:1",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "arn:aws:states:us-east-1:000000000000:stateMachine:edge-greeter:1",
        },
        Case {
            case: "not-an-arn-at-all",
            associations: &[Association {
                arn: "edge-greeter:1",
                event_type: "viewer-request",
            }],
            accepted: false,
            refused_arn: "edge-greeter:1",
        },
        Case {
            // Every association is held to the rule, not just the first one, and the
            // refusal names the entry that broke it rather than the first entry.
            case: "second-association-unqualified",
            associations: &[
                Association {
                    arn: VERSION_ONE,
                    event_type: "viewer-request",
                },
                Association {
                    arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-logger",
                    event_type: "origin-request",
                },
            ],
            accepted: false,
            refused_arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-logger",
        },
    ]
}

/// Every `LambdaFunctionARN` the response reports, wherever it carries one.
pub(crate) fn reported_arns(answer: &Answer) -> Vec<String> {
    fn walk(node: &crate::distribution_lifecycle::Node, out: &mut Vec<String>) {
        if node.children.is_empty() {
            return;
        }
        for association in node.children_named("LambdaFunctionAssociation") {
            if let Some(arn) = association.text_at("LambdaFunctionARN") {
                out.push(arn.to_string());
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(answer.root(), &mut out);
    out
}

#[tokio::test]
async fn an_edge_association_is_stored_only_for_a_published_function_version() {
    let router = emulator();

    for (index, case) in cases().into_iter().enumerate() {
        for behavior in [Behavior::Default, Behavior::Ordered] {
            for (path, answer) in [
                (
                    "CreateDistribution",
                    create_distribution(
                        &router,
                        &config_xml(
                            &format!("ref-{index}-{behavior:?}-plain"),
                            behavior,
                            case.associations,
                        ),
                    )
                    .await,
                ),
                (
                    "CreateDistributionWithTags",
                    create_distribution_with_tags(
                        &router,
                        &config_xml(
                            &format!("ref-{index}-{behavior:?}-tagged"),
                            behavior,
                            case.associations,
                        ),
                    )
                    .await,
                ),
            ] {
                if case.accepted {
                    assert_eq!(
                        answer.status, 201,
                        "case {} ({path}, {behavior:?}): expected HTTP 201, got {} with {}",
                        case.case, answer.status, answer.raw
                    );
                    let arns = reported_arns(&answer);
                    let expected: Vec<String> = case
                        .associations
                        .iter()
                        .map(|association| association.arn.to_string())
                        .collect();
                    assert_eq!(
                        arns, expected,
                        "case {} ({path}, {behavior:?}): expected the created distribution \
                         to carry the submitted associations {expected:?}, got {arns:?}",
                        case.case
                    );
                    continue;
                }
                assert_eq!(
                    answer.status, 400,
                    "case {} ({path}, {behavior:?}): expected HTTP 400, got {} with {}",
                    case.case, answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidLambdaFunctionAssociation",
                    "case {} ({path}, {behavior:?}): expected \
                     InvalidLambdaFunctionAssociation, got {:?} with {}",
                    case.case,
                    answer.error_code(),
                    answer.raw
                );
                let message = answer.root().find_text("Message").unwrap_or_default();
                let expected = refusal_message(case.refused_arn);
                assert_eq!(
                    message, expected,
                    "case {} ({path}, {behavior:?}): expected the refusal to state the \
                     version requirement and name the refused association, got {message:?}",
                    case.case
                );
            }
        }
    }

    // Nothing a refused create named exists: no stored distribution carries an ARN a
    // refused row submitted, and the refusals cost no id. One page carries them all
    // (`MaxItems` admits far more than the accepted rows create). The listing is read
    // through the parsed associations rather than as a substring, because an unqualified
    // ARN is a PREFIX of the version ARN an accepted row carries.
    let listed = list_distributions(&router, &format!("?MaxItems={MAX_ITEMS}")).await;
    assert_eq!(
        listed.status, 200,
        "case listing: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let stored = reported_arns(&listed);
    for case in cases().iter().filter(|case| !case.accepted) {
        for association in case.associations {
            let accepted_elsewhere = cases()
                .iter()
                .filter(|case| case.accepted)
                .flat_map(|case| case.associations)
                .any(|accepted| accepted.arn == association.arn);
            if accepted_elsewhere {
                continue;
            }
            assert!(
                !stored.iter().any(|arn| arn == association.arn),
                "case listing: no distribution may carry the refused association {:?} \
                 of case {}, got {stored:?}",
                association.arn,
                case.case
            );
        }
    }
}

/// The same rule on the UPDATE path, and the round trip: a distribution created on a
/// published version reports that association on a read back, cannot be reconfigured onto
/// an unpublished one, and keeps its stored configuration and version when that update is
/// refused.
#[tokio::test]
async fn an_update_cannot_move_an_association_off_a_published_version() {
    let router = emulator();

    let created = create_distribution(
        &router,
        &config_xml(
            "ref-update",
            Behavior::Default,
            &[Association {
                arn: VERSION_ONE,
                event_type: "viewer-request",
            }],
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case update-setup: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case created-read-back: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        reported_arns(&read),
        vec![VERSION_ONE.to_string()],
        "case created-read-back: expected the stored association to name {VERSION_ONE:?}, \
         got {:?}",
        reported_arns(&read)
    );

    for (case, arn) in [
        (
            "update-to-unqualified",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter",
        ),
        (
            "update-to-latest",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:$LATEST",
        ),
        (
            "update-to-alias",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:prod",
        ),
    ] {
        let refused = update_distribution(
            &router,
            &id,
            &config_xml(
                "ref-update",
                Behavior::Default,
                &[Association {
                    arn,
                    event_type: "viewer-request",
                }],
            ),
            Some(&etag),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidLambdaFunctionAssociation",
            "case {case}: expected InvalidLambdaFunctionAssociation, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        let expected = refusal_message(arn);
        assert_eq!(
            message, expected,
            "case {case}: expected the refusal to name the submitted association, got \
             {message:?}"
        );

        let read = get_distribution(&router, &id).await;
        assert_eq!(
            reported_arns(&read),
            vec![VERSION_ONE.to_string()],
            "case {case}: a refused update must leave the stored association in place, \
             got {:?}",
            reported_arns(&read)
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: a refused update mints no new version, got {:?}",
            read.header("etag")
        );
    }

    // Moving the association onto ANOTHER published version is the reconfiguration
    // CloudFront accepts, and the read back reports it.
    let next_version = "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:2";
    let accepted = update_distribution(
        &router,
        &id,
        &config_xml(
            "ref-update",
            Behavior::Default,
            &[Association {
                arn: next_version,
                event_type: "viewer-request",
            }],
        ),
        Some(&etag),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case update-to-another-version: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert_eq!(
        reported_arns(&accepted),
        vec![next_version.to_string()],
        "case update-to-another-version: expected the association to name \
         {next_version:?}, got {:?}",
        reported_arns(&accepted)
    );
}
