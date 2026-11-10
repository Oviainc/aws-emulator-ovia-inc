// SPDX-License-Identifier: CC0-1.0

//! A distribution's CONNECTION MODE is fixed when the distribution is created, end to end
//! over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: `ConnectionMode` "specifies whether the connection mode is through a
//! standard distribution (direct) or a multi-tenant distribution with distribution tenants
//! (tenant-only)" (`smithy_query(query="shape",shape="ConnectionMode,DistributionConfig")`),
//! which is what the distribution IS rather than a setting of how it serves — so it belongs
//! to the create, and an `UpdateDistribution` whose document declares a different mode is
//! refused `IllegalUpdate` (HTTP 400) with one fixed sentence, writing nothing; an update
//! declaring the SAME mode lands as any other does.
//!
//! What a document DECLARES is the member when it states one, and otherwise the mode its
//! other members describe — `TenantConfig` "only supports multi-tenant distributions" — so
//! the rows below span both values of the member, the member left out entirely, and the
//! tenancy declared only by `TenantConfig`, in both directions.
//!
//! The wire harness is the lifecycle suite's, reused rather than copied; the configuration
//! builder below is `pub(crate)` so the tenant-parameter suite builds its documents here
//! rather than growing a copy.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::ILLEGAL_UPDATE_CONNECTION_MODE_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, update_distribution, Answer,
    API_VERSION,
};

/// `ConnectionMode`'s values, from the model's enum
/// (`smithy_query(query="shape",shape="ConnectionMode")`): exactly these two.
pub(crate) const DIRECT: &str = "direct";
pub(crate) const TENANT_ONLY: &str = "tenant-only";

/// Read one distribution's stored configuration: `GET /<version>/distribution/<id>/config`.
pub(crate) async fn get_distribution_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await
}

/// One `DistributionConfig` document, varying along what the tenancy rules generalise over:
/// whether the `ConnectionMode` member is stated and with which value, whether a
/// `TenantConfig` declares parameters, and what the single origin's domain name and path are.
///
/// The members are emitted in the order `DistributionConfig` declares them
/// (`smithy_query(query="shape",shape="DistributionConfig")`).
#[derive(Debug, Clone)]
pub(crate) struct TenancyConfig {
    pub(crate) caller_reference: String,
    /// `Origin$DomainName` of the single origin.
    pub(crate) domain_name: String,
    /// `Origin$OriginPath`, or `None` to leave the member out entirely.
    pub(crate) origin_path: Option<String>,
    /// The comment, which is what an update changes when it changes anything else.
    pub(crate) comment: String,
    /// The names `TenantConfig$ParameterDefinitions` declares, or `None` to leave
    /// `TenantConfig` out entirely.
    pub(crate) parameters: Option<Vec<&'static str>>,
    /// The `ConnectionMode` member, or `None` to leave it out entirely.
    pub(crate) connection_mode: Option<&'static str>,
}

impl TenancyConfig {
    /// A configuration under `reference` with one origin and no tenancy members at all.
    pub(crate) fn new(reference: &str) -> Self {
        Self {
            caller_reference: reference.to_string(),
            domain_name: "origin.example.com".to_string(),
            origin_path: None,
            comment: "the configuration under test".to_string(),
            parameters: None,
            connection_mode: None,
        }
    }

    /// The document a client sends.
    ///
    /// `PriceClass` is stated on a STANDARD distribution's document, because an update
    /// submits the configuration WHOLE and one that leaves the member out is refused as
    /// incomplete — and left out of a MULTI-TENANT one, where the member "only supports
    /// standard distributions. You can't specify this field for multi-tenant
    /// distributions", so stating it is refused.
    pub(crate) fn xml(&self) -> String {
        let multi_tenant =
            self.connection_mode == Some(TENANT_ONLY) || self.parameters.is_some();
        let mut out = format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>\
             <Origins><Quantity>1</Quantity><Items><Origin><Id>primary</Id>\
             <DomainName>{}</DomainName>",
            self.caller_reference, self.domain_name
        );
        if let Some(origin_path) = &self.origin_path {
            out.push_str(&format!("<OriginPath>{origin_path}</OriginPath>"));
        }
        out.push_str(&format!(
            "</Origin></Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
             <Comment>{}</Comment>{}\
             <Enabled>true</Enabled>",
            self.comment,
            if multi_tenant {
                ""
            } else {
                "<PriceClass>PriceClass_All</PriceClass>"
            }
        ));
        // `ParameterDefinitions` is a list whose member carries no `xmlName`
        // (`smithy_query(query="shape",shape="ParameterDefinitions")`), so each entry travels
        // in the restXml default `<member>` element.
        if let Some(parameters) = &self.parameters {
            out.push_str("<TenantConfig><ParameterDefinitions>");
            for name in parameters {
                out.push_str(&format!(
                    "<member><Name>{name}</Name><Definition><StringSchema>\
                     <Required>false</Required></StringSchema></Definition></member>"
                ));
            }
            out.push_str("</ParameterDefinitions></TenantConfig>");
        }
        if let Some(connection_mode) = self.connection_mode {
            out.push_str(&format!(
                "<ConnectionMode>{connection_mode}</ConnectionMode>"
            ));
        }
        out.push_str("</DistributionConfig>");
        out
    }
}

/// How a row DECLARES its tenancy: the `ConnectionMode` member it states (if any) and
/// whether it carries a `TenantConfig`.
#[derive(Debug, Clone, Copy)]
struct Declaration {
    case: &'static str,
    connection_mode: Option<&'static str>,
    tenant_config: bool,
}

/// The four ways a document declares its tenancy, each named by the mode it declares.
const DECLARATIONS: &[Declaration] = &[
    Declaration {
        case: "the member stating direct",
        connection_mode: Some(DIRECT),
        tenant_config: false,
    },
    Declaration {
        case: "no tenancy member at all",
        connection_mode: None,
        tenant_config: false,
    },
    Declaration {
        case: "the member stating tenant-only",
        connection_mode: Some(TENANT_ONLY),
        tenant_config: true,
    },
    Declaration {
        case: "a TenantConfig and no member",
        connection_mode: None,
        tenant_config: true,
    },
];

impl Declaration {
    /// The mode this declaration declares: the member when it states one, and otherwise the
    /// mode `TenantConfig`'s presence describes.
    fn declares(&self) -> &'static str {
        match self.connection_mode {
            Some(mode) => mode,
            None if self.tenant_config => TENANT_ONLY,
            None => DIRECT,
        }
    }

    /// This declaration's document under `reference`, carrying `comment`.
    fn config(&self, reference: &str, comment: &str) -> TenancyConfig {
        TenancyConfig {
            comment: comment.to_string(),
            parameters: self.tenant_config.then(|| vec!["tenantName"]),
            connection_mode: self.connection_mode,
            ..TenancyConfig::new(reference)
        }
    }
}

/// Every ordered pair of declarations, as `(created, updated)` — the whole class the rule
/// ranges over, which includes the pairs that declare the same mode in different words.
fn transitions() -> Vec<(Declaration, Declaration)> {
    DECLARATIONS
        .iter()
        .flat_map(|created| DECLARATIONS.iter().map(move |updated| (*created, *updated)))
        .collect()
}

/// An update declaring another connection mode is refused with the one fixed sentence and
/// writes nothing; one declaring the same mode lands.
#[tokio::test]
async fn an_update_cannot_move_a_distribution_between_connection_modes() {
    for (created, updated) in transitions() {
        let case = format!("created with {}, updated with {}", created.case, updated.case);
        let router = emulator();

        let reference = "connection-mode-under-test";
        let stored = created.config(reference, "as created");
        let create = create_distribution(&router, &stored.xml()).await;
        assert_eq!(
            create.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            create.status, create.raw
        );
        let id = create
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let etag = create.header("etag").to_string();

        let submitted = updated.config(reference, "as updated");
        let answer = update_distribution(&router, &id, &submitted.xml(), Some(&etag)).await;

        if created.declares() == updated.declares() {
            assert_eq!(
                answer.status, 200,
                "case {case}: both documents declare {}, so the update is expected to land, got \
                 {} with {}",
                created.declares(),
                answer.status,
                answer.raw
            );
            let read = get_distribution_config(&router, &id).await;
            assert_eq!(
                read.root().text_at("Comment"),
                Some("as updated"),
                "case {case}: expected the update to have been stored, got {}",
                read.raw
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "IllegalUpdate",
            "case {case}: expected IllegalUpdate, got {:?} in {}",
            answer.error_code(),
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, ILLEGAL_UPDATE_CONNECTION_MODE_MESSAGE,
            "case {case}: expected the fixed connection-mode sentence, got {message:?}"
        );
        assert!(
            !message.contains(&id) && !message.contains(created.declares()),
            "case {case}: expected a sentence carrying nothing from the request or the record, \
             got {message:?}"
        );

        // Nothing was written: the stored configuration is still the one the create stored,
        // at the version the refused update matched on.
        let read = get_distribution_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 reading the configuration back, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Comment"),
            Some("as created"),
            "case {case}: expected the stored configuration to be untouched, got {}",
            read.raw
        );
        assert_eq!(
            read.root().text_at("ConnectionMode"),
            created.connection_mode,
            "case {case}: expected the stored ConnectionMode to be untouched, got {}",
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the version token to be untouched, got {:?}",
            read.header("etag")
        );
        let distribution = get_distribution(&router, &id).await;
        assert_eq!(
            distribution.status, 200,
            "case {case}: expected the distribution to still be readable, got {} with {}",
            distribution.status, distribution.raw
        );
    }
}
