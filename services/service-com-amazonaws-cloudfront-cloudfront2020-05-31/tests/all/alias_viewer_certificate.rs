// SPDX-License-Identifier: CC0-1.0

//! The rule that binds a distribution's ALTERNATE DOMAIN NAMES to its VIEWER CERTIFICATE,
//! end to end over HTTP.
//!
//! `ViewerCertificate` states it in both directions: "If the distribution doesn't use
//! `Aliases` … set `CloudFrontDefaultCertificate` to `true` and leave all other fields
//! empty", and a distribution that DOES use them names one of `ACMCertificateArn` or
//! `IAMCertificateId` — a certificate that validates the authorization to serve the domain
//! name, which CloudFront's own `*.cloudfront.net` certificate cannot do for a name that is
//! not under it. So: a configuration that declares at least one alias and names exactly one
//! custom certificate is created; one that declares an alias and names no custom
//! certificate (the default certificate, by itself or by omission), or names two, is
//! refused with `InvalidViewerCertificate` (HTTP 400) and NOTHING is created.
//!
//! What the rows span: zero, one, two and three aliases; a certificate named by ACM ARN and
//! by IAM certificate id; the default certificate stated explicitly, left out entirely and
//! stated beside a custom one; both custom members at once; a custom member left empty
//! (which names no certificate); and both paths that store a configuration — the two
//! creates and the update.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, list_distributions,
    update_distribution, Answer, API_VERSION,
};

/// An ACM certificate ARN: a certificate kept in AWS Certificate Manager, which is what
/// `ViewerCertificate$ACMCertificateArn` names ("the ARN … in AWS Certificate Manager
/// (ACM) … in the US East (N. Virginia) Region").
const ACM_ARN: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// An IAM certificate id: the other half of the same choice
/// (`ViewerCertificate$IAMCertificateId`, "the ID of the certificate … stored in IAM").
const IAM_CERTIFICATE_ID: &str = "ASCACKCEVSQ6CEXAMPLE1";

/// The `ViewerCertificate` a row submits, or `None` for a configuration that names the
/// element not at all.
#[derive(Debug, Clone, Copy)]
struct Certificate {
    default: Option<bool>,
    acm: Option<&'static str>,
    iam: Option<&'static str>,
}

impl Certificate {
    /// The element as XML, in the member order the shape declares.
    fn xml(&self) -> String {
        let mut out = String::from("<ViewerCertificate>");
        if let Some(default) = self.default {
            out.push_str(&format!(
                "<CloudFrontDefaultCertificate>{default}</CloudFrontDefaultCertificate>"
            ));
        }
        if let Some(iam) = self.iam {
            out.push_str(&format!("<IAMCertificateId>{iam}</IAMCertificateId>"));
        }
        if let Some(acm) = self.acm {
            out.push_str(&format!("<ACMCertificateArn>{acm}</ACMCertificateArn>"));
        }
        if self.acm.is_some() || self.iam.is_some() {
            out.push_str("<SSLSupportMethod>sni-only</SSLSupportMethod>");
            out.push_str("<MinimumProtocolVersion>TLSv1.2_2021</MinimumProtocolVersion>");
        }
        out.push_str("</ViewerCertificate>");
        out
    }
}

/// The default certificate, stated explicitly.
const DEFAULT_CERTIFICATE: Certificate = Certificate {
    default: Some(true),
    acm: None,
    iam: None,
};

/// A certificate kept in ACM.
const ACM_CERTIFICATE: Certificate = Certificate {
    default: None,
    acm: Some(ACM_ARN),
    iam: None,
};

/// The `DistributionConfig` body for `reference`, with `aliases` and `certificate`.
fn config_xml(reference: &str, aliases: &[&str], certificate: Option<Certificate>) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>"
    );
    if !aliases.is_empty() {
        out.push_str(&format!(
            "<Aliases><Quantity>{}</Quantity><Items>",
            aliases.len()
        ));
        for alias in aliases {
            out.push_str(&format!("<CNAME>{alias}</CNAME>"));
        }
        out.push_str("</Items></Aliases>");
    }
    out.push_str(
        "<Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig></Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>redirect-to-https</ViewerProtocolPolicy>\
         </DefaultCacheBehavior>",
    );
    out.push_str(&format!("<Comment>for {reference}</Comment>"));
    // An update submits the configuration WHOLE, including the price class, so the document
    // states it: a config that leaves it out is refused as an incomplete update.
    out.push_str("<PriceClass>PriceClass_All</PriceClass>");
    out.push_str("<Enabled>true</Enabled>");
    if let Some(certificate) = certificate {
        out.push_str(&certificate.xml());
    }
    out.push_str("</DistributionConfig>");
    out
}

/// `POST /2020-05-31/distribution?WithTags`, so the rule is asserted on the second create
/// path too — both run the one create.
async fn create_distribution_with_tags(
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

/// One row: the aliases and the certificate a configuration states, and whether CloudFront
/// creates it.
struct Case {
    case: &'static str,
    aliases: &'static [&'static str],
    certificate: Option<Certificate>,
    /// `true` when the configuration is one CloudFront accepts.
    accepted: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no-alias-default-certificate",
            aliases: &[],
            certificate: Some(DEFAULT_CERTIFICATE),
            accepted: true,
        },
        Case {
            case: "no-alias-no-certificate-element",
            aliases: &[],
            certificate: None,
            accepted: true,
        },
        Case {
            // A distribution with no alias may still carry a custom certificate: the rule
            // constrains what an ALIAS requires, not what a certificate allows.
            case: "no-alias-acm-certificate",
            aliases: &[],
            certificate: Some(ACM_CERTIFICATE),
            accepted: true,
        },
        Case {
            case: "one-alias-acm-certificate",
            aliases: &["cdn.example.com"],
            certificate: Some(ACM_CERTIFICATE),
            accepted: true,
        },
        Case {
            case: "two-aliases-acm-certificate",
            aliases: &["cdn.example.com", "www.example.com"],
            certificate: Some(ACM_CERTIFICATE),
            accepted: true,
        },
        Case {
            case: "one-alias-iam-certificate",
            aliases: &["iam.example.com"],
            certificate: Some(Certificate {
                default: None,
                acm: None,
                iam: Some(IAM_CERTIFICATE_ID),
            }),
            accepted: true,
        },
        Case {
            // The default flag stated as FALSE beside a custom certificate is the ordinary
            // spelling an SDK sends.
            case: "one-alias-acm-certificate-default-false",
            aliases: &["explicit.example.com"],
            certificate: Some(Certificate {
                default: Some(false),
                acm: Some(ACM_ARN),
                iam: None,
            }),
            accepted: true,
        },
        Case {
            case: "one-alias-default-certificate",
            aliases: &["refused-default.example.com"],
            certificate: Some(DEFAULT_CERTIFICATE),
            accepted: false,
        },
        Case {
            case: "one-alias-no-certificate-element",
            aliases: &["refused-absent.example.com"],
            certificate: None,
            accepted: false,
        },
        Case {
            case: "three-aliases-default-certificate",
            aliases: &[
                "refused-a.example.com",
                "refused-b.example.com",
                "refused-c.example.com",
            ],
            certificate: Some(DEFAULT_CERTIFICATE),
            accepted: false,
        },
        Case {
            case: "one-alias-default-and-acm-certificate",
            aliases: &["refused-both-kinds.example.com"],
            certificate: Some(Certificate {
                default: Some(true),
                acm: Some(ACM_ARN),
                iam: None,
            }),
            accepted: false,
        },
        Case {
            case: "one-alias-acm-and-iam-certificate",
            aliases: &["refused-two-customs.example.com"],
            certificate: Some(Certificate {
                default: None,
                acm: Some(ACM_ARN),
                iam: Some(IAM_CERTIFICATE_ID),
            }),
            accepted: false,
        },
        Case {
            // An empty member names no certificate — it is the "leave all other fields
            // empty" spelling of the default certificate.
            case: "one-alias-empty-acm-certificate",
            aliases: &["refused-empty.example.com"],
            certificate: Some(Certificate {
                default: None,
                acm: Some(""),
                iam: None,
            }),
            accepted: false,
        },
    ]
}

/// Every alias in the response's `Aliases/Items`.
fn reported_aliases(answer: &Answer) -> Vec<String> {
    answer
        .root()
        .at("DistributionConfig/Aliases/Items")
        .map(|items| {
            items
                .children_named("CNAME")
                .into_iter()
                .map(|alias| alias.text.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn an_alias_is_created_only_under_a_certificate_that_covers_it() {
    let router = emulator();

    for (index, case) in cases().into_iter().enumerate() {
        for (path, answer) in [
            (
                "CreateDistribution",
                create_distribution(
                    &router,
                    &config_xml(
                        &format!("ref-{index}-plain"),
                        case.aliases,
                        case.certificate,
                    ),
                )
                .await,
            ),
            (
                "CreateDistributionWithTags",
                create_distribution_with_tags(
                    &router,
                    &config_xml(
                        &format!("ref-{index}-tagged"),
                        case.aliases,
                        case.certificate,
                    ),
                )
                .await,
            ),
        ] {
            if case.accepted {
                assert_eq!(
                    answer.status, 201,
                    "case {} ({path}): expected HTTP 201, got {} with {}",
                    case.case, answer.status, answer.raw
                );
                let aliases = reported_aliases(&answer);
                assert_eq!(
                    aliases,
                    case.aliases
                        .iter()
                        .map(|alias| (*alias).to_string())
                        .collect::<Vec<String>>(),
                    "case {} ({path}): expected the created distribution to carry the \
                     submitted aliases {:?}, got {aliases:?}",
                    case.case,
                    case.aliases
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {} ({path}): expected HTTP 400, got {} with {}",
                case.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidViewerCertificate",
                "case {} ({path}): expected InvalidViewerCertificate, got {:?} with {}",
                case.case,
                answer.error_code(),
                answer.raw
            );
        }
    }

    // And nothing a refused create named exists: no distribution in the store carries an
    // alias of a refused row, and the refusals cost no id — only the accepted rows are
    // there. One page carries them all (`MaxItems` admits far more).
    let listed = list_distributions(&router, &format!("?MaxItems={MAX_ITEMS}")).await;
    assert_eq!(
        listed.status, 200,
        "case listing: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let refused_aliases: Vec<&str> = cases()
        .iter()
        .filter(|case| !case.accepted)
        .flat_map(|case| case.aliases.iter().copied())
        .collect();
    for alias in refused_aliases {
        assert!(
            !listed.raw.contains(alias),
            "case listing: no distribution may carry the refused alias {alias:?}, \
             got {}",
            listed.raw
        );
    }
}

/// The same rule on the UPDATE path: a distribution that was created with a certificate
/// cannot be reconfigured onto the default one while it still declares an alias, and the
/// refused update leaves the stored configuration exactly as it was.
#[tokio::test]
async fn an_update_cannot_drop_the_certificate_an_alias_needs() {
    let router = emulator();

    let created = create_distribution(
        &router,
        &config_xml("ref-update", &["update.example.com"], Some(ACM_CERTIFICATE)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case update-setup: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    let refused = update_distribution(
        &router,
        &id,
        &config_xml(
            "ref-update",
            &["update.example.com"],
            Some(DEFAULT_CERTIFICATE),
        ),
        Some(&etag),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case update-to-default-certificate: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InvalidViewerCertificate",
        "case update-to-default-certificate: expected InvalidViewerCertificate, got {:?}",
        refused.error_code()
    );

    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case update-refused-read-back: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root()
            .text_at("DistributionConfig/ViewerCertificate/ACMCertificateArn"),
        Some(ACM_ARN),
        "case update-refused-read-back: the refused update must leave the stored \
         certificate in place, got {}",
        read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case update-refused-read-back: a refused update mints no new version, got {:?}",
        read.header("etag")
    );

    // Dropping the ALIAS and the certificate together is the configuration
    // `ViewerCertificate` describes for a distribution with no alternate domain name, so
    // that update lands.
    let accepted = update_distribution(
        &router,
        &id,
        &config_xml("ref-update", &[], Some(DEFAULT_CERTIFICATE)),
        Some(&etag),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case update-drops-alias-and-certificate: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert!(
        reported_aliases(&accepted).is_empty(),
        "case update-drops-alias-and-certificate: expected no alias, got {:?}",
        reported_aliases(&accepted)
    );
}
