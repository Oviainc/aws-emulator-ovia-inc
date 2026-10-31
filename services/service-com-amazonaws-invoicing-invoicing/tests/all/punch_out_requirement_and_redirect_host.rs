// SPDX-License-Identifier: CC0-1.0

//! The two Marketplace PunchOut preconditions a write of a procurement portal preference
//! applies, driven end to end over this service's wire protocol: `POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, through the same
//! `MultiServiceRouter` the production router uses, so what these tests see is what an SDK
//! client sees.
//!
//! RULE 1 — a write that leaves Marketplace PunchOut enabled has to state the Marketplace
//! PunchOut configuration, as far as its OWN input member states that requirement:
//! `PutProcurementPortalPreferenceRequest$MarketplacePunchOutPreference` is "required when
//! MarketplacePunchOutEnabled is true" with no portal qualification, so every portal's
//! update needs it, and a configuration an earlier write stored is no substitute;
//! `CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference` is "required
//! for Coupa when MarketplacePunchOutEnabled is true", so the create needs it for that one
//! portal (`smithy_query(query="doc", shape="<Request>",
//! member="MarketplacePunchOutPreference")`).
//!
//! RULE 2 — `MarketplacePunchOutPreference$ApprovalRequestRedirectUrl`: "when provided
//! together with the procurement portal instance endpoint, its host must match the host of
//! that endpoint" (`smithy_query(query="doc", shape="MarketplacePunchOutPreference",
//! member="ApprovalRequestRedirectUrl")`), so no write may leave a preference holding a
//! redirect URL on one host and a `ProcurementPortalInstanceEndpoint` on another — and
//! only the HOST is compared, so the scheme, the port, the path and the query are each
//! endpoint's own.
//!
//! Each table spans the class rather than one request: rule 1 over the whole portal
//! catalog, the enable flag stated true, stated false and omitted, the configuration
//! stated and omitted, and a preference that already holds a configuration as well as one
//! that does not; rule 2 over host pairs that agree, agree but for the port, the path, the
//! query or the spelling's case, and disagree in the host, the subdomain or the registered
//! domain — each driven through BOTH writes, and every accepted row read back through
//! `GetProcurementPortalPreference`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    arn_of, call, emulator_with_engine, error_code, minimal_create, ok, PORTAL_NAMES,
};

/// The one portal the CREATE's own member states its configuration requirement for.
const PORTAL_THE_CREATE_REQUIRES_CONFIG_FOR: &str = "COUPA";

/// The two members the refusal of rule 1 is about.
const PUNCH_OUT_CONFIG_MEMBER: &str = "MarketplacePunchOutPreference";
const PUNCH_OUT_FLAG_MEMBER: &str = "MarketplacePunchOutEnabled";

/// The key every refusal of the procurement-portal-preference plane carries its
/// explanatory text under.
const PORTAL_PLANE_KEY: &str = "Message";

/// The explanatory text of a refusal of this plane, if the body carries one.
fn message(body: &Value) -> String {
    body.get(PORTAL_PLANE_KEY)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `MarketplacePunchOutPreference` whose one member
/// (`MarketplacePunchOutPreference$ApprovalRequestRedirectUrl`, a `^\S+$` string) is `url`.
fn punch_out_config(url: &str) -> Value {
    json!({"ApprovalRequestRedirectUrl": url})
}

/// A `PutProcurementPortalPreference` request for `arn`, stating every member the
/// operation marks required and nothing else.
fn put_request(arn: &str) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": [{"Name": "Portal Administrator", "Email": "portal.admin@example.com"}],
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// The preference `arn` names, as `GetProcurementPortalPreference` reports it.
async fn get_preference(router: &MultiServiceRouter, arn: &str) -> Value {
    let read = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    read.get("ProcurementPortalPreference")
        .cloned()
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"))
}

/// One row of the requirement table.
#[derive(Debug)]
struct RequirementRow {
    case: String,
    portal: &'static str,
    /// `MarketplacePunchOutEnabled` as the request states it, or `None` for a request that
    /// omits it — the case the documented default (`false`) covers.
    enabled: Option<bool>,
    /// Whether the request states a `MarketplacePunchOutPreference` of its own.
    states_config: bool,
    /// Whether the preference the UPDATE acts on already holds a configuration, which the
    /// requirement is not satisfied by.
    already_holds_config: bool,
}

impl RequirementRow {
    /// Whether the UPDATE refuses this row: enabled without a stated configuration, for
    /// any portal.
    fn put_refused(&self) -> bool {
        self.enabled == Some(true) && !self.states_config
    }

    /// Whether the CREATE refuses this row: enabled without a stated configuration, for
    /// the one portal its own member names.
    fn create_refused(&self) -> bool {
        self.enabled == Some(true)
            && !self.states_config
            && self.portal == PORTAL_THE_CREATE_REQUIRES_CONFIG_FOR
    }
}

/// Every combination of the members rule 1 reads.
fn requirement_rows() -> Vec<RequirementRow> {
    let mut rows = Vec::new();
    for &portal in PORTAL_NAMES {
        for enabled in [Some(true), Some(false), None] {
            for states_config in [false, true] {
                for already_holds_config in [false, true] {
                    rows.push(RequirementRow {
                        case: format!(
                            "{portal}, enabled {enabled:?}, config stated {states_config}, \
                             already holds one {already_holds_config}"
                        ),
                        portal,
                        enabled,
                        states_config,
                        already_holds_config,
                    });
                }
            }
        }
    }
    rows
}

/// The redirect URL a requirement row states, on no host any endpoint of these rows names
/// — rule 1 is about the configuration being STATED, and these rows state no instance
/// endpoint for rule 2 to be about.
const REQUIREMENT_REDIRECT_URL: &str = "https://portal.example.com/approve";

/// RULE 1, over the whole portal catalog: the update refuses an enabled Marketplace
/// PunchOut whose configuration it does not state, whatever the portal and whatever the
/// preference already holds, while the create refuses it for the one portal its own member
/// names — and no accepted write leaves a preference reporting the flag enabled with no
/// configuration behind it, the combination
/// `ProcurementPortalPreference$MarketplacePunchOutPreference` documents as present
/// whenever the flag is true.
#[tokio::test]
async fn an_enabled_punch_out_needs_the_configuration_its_own_member_requires() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in requirement_rows().iter().enumerate() {
        let case = &row.case;

        // The preference the UPDATE acts on: one identity per row, so a conflicting create
        // never stands in for the rule's answer.
        let mut baseline = minimal_create(
            row.portal,
            &format!("update-buyer-{index}"),
            &format!("update-supplier-{index}"),
        );
        if row.already_holds_config {
            baseline[PUNCH_OUT_FLAG_MEMBER] = json!(true);
            baseline[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(REQUIREMENT_REDIRECT_URL);
        }
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &baseline).await);
        let before = get_preference(&router, &arn).await;

        let mut update = put_request(&arn);
        if let Some(enabled) = row.enabled {
            update[PUNCH_OUT_FLAG_MEMBER] = json!(enabled);
        }
        if row.states_config {
            update[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(REQUIREMENT_REDIRECT_URL);
        }
        let (put_status, put_body) =
            call(&router, "PutProcurementPortalPreference", &update).await;

        if row.put_refused() {
            assert_eq!(
                put_status, 400,
                "case {case}: the update expected HTTP 400, got {put_status} {put_body}"
            );
            assert_eq!(
                error_code(&put_body),
                "ValidationException",
                "case {case}: the update expected a ValidationException, got {put_body}"
            );
            let text = message(&put_body);
            assert!(
                text.contains(PUNCH_OUT_CONFIG_MEMBER) && text.contains(PUNCH_OUT_FLAG_MEMBER),
                "case {case}: the refusal must name the two members whose combination it \
                 refuses, got {text:?}"
            );
            let after = get_preference(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update writes nothing, so the preference must still \
                 report what its create left, got {after}"
            );
        } else {
            assert_eq!(
                put_status, 200,
                "case {case}: the update expected HTTP 200, got {put_status} {put_body}"
            );
            let after = get_preference(&router, &arn).await;
            let enabled = after.get(PUNCH_OUT_FLAG_MEMBER).and_then(Value::as_bool);
            assert_eq!(
                enabled,
                Some(row.enabled.unwrap_or(false)),
                "case {case}: expected the stated flag or its documented default, got {after}"
            );
            assert_eq!(
                after.get(PUNCH_OUT_CONFIG_MEMBER).is_some(),
                enabled == Some(true),
                "case {case}: a configuration is reported exactly while the flag is true, \
                 got {after}"
            );
        }

        // The same row through the CREATE, which states its portal on its own input and
        // applies the narrower scope its own member states.
        let mut created = minimal_create(
            row.portal,
            &format!("create-buyer-{index}"),
            &format!("create-supplier-{index}"),
        );
        if let Some(enabled) = row.enabled {
            created[PUNCH_OUT_FLAG_MEMBER] = json!(enabled);
        }
        if row.states_config {
            created[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(REQUIREMENT_REDIRECT_URL);
        }
        let (create_status, create_body) =
            call(&router, "CreateProcurementPortalPreference", &created).await;
        let expected_create_status = if row.create_refused() { 400 } else { 200 };
        assert_eq!(
            create_status, expected_create_status,
            "case {case}: the create expected HTTP {expected_create_status}, got \
             {create_status} {create_body}"
        );
        if row.create_refused() {
            let text = message(&create_body);
            assert!(
                text.contains(row.portal),
                "case {case}: the create's refusal names the portal its requirement is \
                 stated for, got {text:?}"
            );
        }
    }
}

/// One row of the host table: the two endpoints a write states, and whether their hosts
/// are the same host.
#[derive(Debug)]
struct HostRow {
    case: &'static str,
    /// `ProcurementPortalInstanceEndpoint`, or `None` for a write that states none.
    endpoint: Option<&'static str>,
    /// `MarketplacePunchOutPreference$ApprovalRequestRedirectUrl`.
    redirect_url: &'static str,
    /// Whether the pair names one host, which is what the rule accepts.
    hosts_match: bool,
}

/// The host pairs the rule reads: agreeing in the host while differing in the port, the
/// path, the query and the spelling's case, and disagreeing in the host, the subdomain and
/// the registered domain — plus a write that states no instance endpoint at all, where
/// there is no pair for the rule to be about.
fn host_rows() -> Vec<HostRow> {
    vec![
        HostRow {
            case: "the same host and path",
            endpoint: Some("https://portal.example.com/cxml"),
            redirect_url: "https://portal.example.com/cxml",
            hosts_match: true,
        },
        HostRow {
            case: "the same host, different paths",
            endpoint: Some("https://portal.example.com/cxml"),
            redirect_url: "https://portal.example.com/approvals/new",
            hosts_match: true,
        },
        HostRow {
            case: "the same host, different ports",
            endpoint: Some("https://portal.example.com:8443/cxml"),
            redirect_url: "https://portal.example.com/approvals",
            hosts_match: true,
        },
        HostRow {
            case: "the same host, a query on one of them",
            endpoint: Some("https://portal.example.com/cxml?v=1"),
            redirect_url: "https://portal.example.com/approvals",
            hosts_match: true,
        },
        HostRow {
            case: "the same host spelled in different case",
            endpoint: Some("https://PORTAL.Example.COM/cxml"),
            redirect_url: "https://portal.example.com/approvals",
            hosts_match: true,
        },
        HostRow {
            case: "another registered domain",
            endpoint: Some("https://portal.example.com/cxml"),
            redirect_url: "https://other.example.net/approvals",
            hosts_match: false,
        },
        HostRow {
            case: "another subdomain of the same domain",
            endpoint: Some("https://portal.example.com/cxml"),
            redirect_url: "https://approvals.example.com/approvals",
            hosts_match: false,
        },
        HostRow {
            case: "the host as a prefix of the other",
            endpoint: Some("https://portal.example.com/cxml"),
            redirect_url: "https://portal.example.com.evil.test/approvals",
            hosts_match: false,
        },
        HostRow {
            case: "no instance endpoint for the redirect URL to be together with",
            endpoint: None,
            redirect_url: "https://portal.example.com/approvals",
            hosts_match: true,
        },
    ]
}

/// RULE 2, through BOTH writes: a write that would leave a preference holding an approval
/// redirect URL whose host is not the host of its instance endpoint is the modelled
/// `ValidationException`, and a write whose hosts agree — however their port, path, query
/// or case differ — is applied and read back.
#[tokio::test]
async fn an_approval_redirect_url_must_share_the_host_of_the_instance_endpoint() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in host_rows().iter().enumerate() {
        let case = row.case;
        let expected_status = if row.hosts_match { 200 } else { 400 };

        // THE CREATE: it states both endpoints on its own input. A portal the create's own
        // requirement names, so the row carries a stated configuration either way.
        let mut created = minimal_create(
            PORTAL_THE_CREATE_REQUIRES_CONFIG_FOR,
            &format!("host-create-buyer-{index}"),
            &format!("host-create-supplier-{index}"),
        );
        created[PUNCH_OUT_FLAG_MEMBER] = json!(true);
        created[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(row.redirect_url);
        if let Some(endpoint) = row.endpoint {
            created["ProcurementPortalInstanceEndpoint"] = json!(endpoint);
        }
        let (create_status, create_body) =
            call(&router, "CreateProcurementPortalPreference", &created).await;
        assert_eq!(
            create_status, expected_status,
            "case {case}: the create expected HTTP {expected_status}, got {create_status} \
             {create_body}"
        );
        if row.hosts_match {
            let reported = get_preference(&router, &arn_of(&create_body)).await;
            assert_eq!(
                reported
                    .get(PUNCH_OUT_CONFIG_MEMBER)
                    .and_then(|config| config.get("ApprovalRequestRedirectUrl"))
                    .and_then(Value::as_str),
                Some(row.redirect_url),
                "case {case}: the create must report the redirect URL it stated, got \
                 {reported}"
            );
        } else {
            assert_eq!(
                error_code(&create_body),
                "ValidationException",
                "case {case}: the create expected a ValidationException, got {create_body}"
            );
        }

        // THE UPDATE, against a preference that holds neither endpoint yet: it states both
        // on its own input, exactly as the create did.
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    PORTAL_THE_CREATE_REQUIRES_CONFIG_FOR,
                    &format!("host-put-buyer-{index}"),
                    &format!("host-put-supplier-{index}"),
                ),
            )
            .await,
        );
        let before = get_preference(&router, &arn).await;
        let mut update = put_request(&arn);
        update[PUNCH_OUT_FLAG_MEMBER] = json!(true);
        update[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(row.redirect_url);
        if let Some(endpoint) = row.endpoint {
            update["ProcurementPortalInstanceEndpoint"] = json!(endpoint);
        }
        let (put_status, put_body) =
            call(&router, "PutProcurementPortalPreference", &update).await;
        assert_eq!(
            put_status, expected_status,
            "case {case}: the update expected HTTP {expected_status}, got {put_status} \
             {put_body}"
        );
        if row.hosts_match {
            let reported = get_preference(&router, &arn).await;
            assert_eq!(
                reported
                    .get(PUNCH_OUT_CONFIG_MEMBER)
                    .and_then(|config| config.get("ApprovalRequestRedirectUrl"))
                    .and_then(Value::as_str),
                Some(row.redirect_url),
                "case {case}: the update must report the redirect URL it stated, got \
                 {reported}"
            );
        } else {
            assert_eq!(
                error_code(&put_body),
                "ValidationException",
                "case {case}: the update expected a ValidationException, got {put_body}"
            );
            let after = get_preference(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update writes nothing, so the preference must still \
                 report what its create left, got {after}"
            );
        }

        // The ENDPOINT a row's update does not state is the one the preference already
        // holds: the pair the rule reads is what the write LEAVES on the resource, not only
        // what one request names. Carried by the rows that state an endpoint, which is what
        // a preference can already hold.
        if let Some(endpoint) = row.endpoint {
            let seeded = arn_of(
                &ok(&router, "CreateProcurementPortalPreference", &{
                    let mut request = minimal_create(
                        PORTAL_THE_CREATE_REQUIRES_CONFIG_FOR,
                        &format!("host-stored-buyer-{index}"),
                        &format!("host-stored-supplier-{index}"),
                    );
                    request["ProcurementPortalInstanceEndpoint"] = json!(endpoint);
                    request
                })
                .await,
            );
            let mut update = put_request(&seeded);
            update[PUNCH_OUT_FLAG_MEMBER] = json!(true);
            update[PUNCH_OUT_CONFIG_MEMBER] = punch_out_config(row.redirect_url);
            let (status, body) = call(&router, "PutProcurementPortalPreference", &update).await;
            assert_eq!(
                status, expected_status,
                "case {case}: an update against the stored endpoint expected HTTP \
                 {expected_status}, got {status} {body}"
            );
        }
    }
}
