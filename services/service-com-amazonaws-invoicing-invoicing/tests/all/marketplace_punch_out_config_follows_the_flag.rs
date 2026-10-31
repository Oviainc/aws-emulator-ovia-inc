// SPDX-License-Identifier: CC0-1.0

//! The Marketplace PunchOut CONFIGURATION a stored procurement portal preference reports,
//! driven end to end over this service's wire protocol through both writes that set it.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, so what
//! these tests see is exactly what an SDK client sees.
//!
//! The rule: `MarketplacePunchOutPreference` is "the Marketplace PunchOut configuration for
//! this procurement portal preference. This is present when `MarketplacePunchOutEnabled` is
//! `true`" (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so the
//! configuration a reader reports belongs to an ENABLED feature: a write that leaves
//! Marketplace PunchOut disabled stores no configuration for it — neither one the request
//! states nor one an earlier write left — and a write that leaves it enabled reports the
//! configuration that write STATES, which is what the requirement each write's own member
//! documents asks it for.
//!
//! The table spans the whole portal catalog, the enable flag stated true, stated false and
//! omitted, the configuration stated and omitted, and — for the update, which acts on a
//! resource that already holds one — a preference that was created WITH a configuration and
//! one created without. Each row drives the create and the update and reads the preference
//! back through `GetProcurementPortalPreference`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    arn_of, call, emulator_with_engine, error_code, minimal_create, ok, PORTAL_NAMES,
};

/// The one portal the CREATE's own member states its configuration requirement for
/// (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`: "Required
/// for Coupa when MarketplacePunchOutEnabled is true"). The UPDATE's member states it with
/// no portal qualification ("Required when MarketplacePunchOutEnabled is true"), so every
/// portal's update needs it.
const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";

/// The `ApprovalRequestRedirectUrl` (`MarketplacePunchOutPreference`'s one member, a
/// `^\S+$` string) of the configuration a ROW's write states.
const STATED_REDIRECT_URL: &str = "https://example.com/stated-approve";

/// The same member of the configuration an EARLIER write stored, worded differently from
/// [`STATED_REDIRECT_URL`] so a read says WHICH configuration is being reported.
const PRE_STORED_REDIRECT_URL: &str = "https://example.com/pre-stored-approve";

/// A `MarketplacePunchOutPreference` carrying `url`.
fn punch_out_config(url: &str) -> Value {
    json!({"ApprovalRequestRedirectUrl": url})
}

/// `request` with the Marketplace PunchOut members a row states written onto it: the flag
/// when the row states one, and the configuration when the row states one.
fn with_punch_out(mut request: Value, enabled: Option<bool>, config: Option<&str>) -> Value {
    if let Some(enabled) = enabled {
        request["MarketplacePunchOutEnabled"] = json!(enabled);
    }
    if let Some(url) = config {
        request["MarketplacePunchOutPreference"] = punch_out_config(url);
    }
    request
}

/// A `PutProcurementPortalPreference` request for `arn` stating every member the operation
/// marks required and nothing of the Marketplace PunchOut configuration.
fn put_request(arn: &str) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": [{"Name": "Portal Administrator", "Email": "portal.admin@example.com"}],
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// What `GetProcurementPortalPreference` reports of one preference's Marketplace PunchOut
/// pair: the flag, and the `ApprovalRequestRedirectUrl` of the configuration when a
/// configuration is reported at all.
#[derive(Debug, PartialEq, Eq)]
struct ReportedPunchOut {
    enabled: Option<bool>,
    redirect_url: Option<String>,
}

/// The Marketplace PunchOut pair a reader reports for `arn`.
async fn reported(router: &MultiServiceRouter, arn: &str) -> ReportedPunchOut {
    let read = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    let preference = read
        .get("ProcurementPortalPreference")
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"))
        .clone();
    let config = preference.get("MarketplacePunchOutPreference").cloned();
    let redirect_url = config.as_ref().map(|config| {
        config
            .get("ApprovalRequestRedirectUrl")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("a reported MarketplacePunchOutPreference states its url, got {config}")
            })
            .to_string()
    });
    ReportedPunchOut {
        enabled: preference
            .get("MarketplacePunchOutEnabled")
            .and_then(Value::as_bool),
        redirect_url,
    }
}

/// One combination of the members the rule reads.
#[derive(Debug)]
struct Row {
    case: String,
    portal: &'static str,
    /// `MarketplacePunchOutEnabled` as the request states it, or `None` for a request that
    /// omits it — the case the documented default (`false`) covers.
    enabled: Option<bool>,
    /// Whether the request states a `MarketplacePunchOutPreference`.
    states_config: bool,
    /// Whether the preference the UPDATE acts on was created holding a configuration.
    pre_stored_config: bool,
}

impl Row {
    /// Whether the CREATE refuses this combination: Marketplace PunchOut enabled, without
    /// the configuration, for the one portal the create's member names.
    fn create_refused(&self) -> bool {
        self.enabled == Some(true)
            && self.portal == PORTAL_REQUIRING_PUNCH_OUT_CONFIG
            && !self.states_config
    }

    /// Whether the UPDATE refuses this combination: Marketplace PunchOut enabled without
    /// the configuration, whatever the portal — the update's member qualifies the
    /// requirement by no portal.
    fn put_refused(&self) -> bool {
        self.enabled == Some(true) && !self.states_config
    }

    /// The flag a reader reports once the write is accepted: what the request states, or
    /// the documented default for one that omits it.
    fn stored_flag(&self) -> bool {
        self.enabled.unwrap_or(false)
    }

    /// What the CREATE leaves for a reader to report: the configuration this request
    /// states, and only while the flag it resolves leaves the feature enabled. A create has
    /// no earlier configuration to keep.
    fn created(&self) -> ReportedPunchOut {
        let redirect_url = (self.stored_flag() && self.states_config)
            .then(|| STATED_REDIRECT_URL.to_string());
        ReportedPunchOut {
            enabled: Some(self.stored_flag()),
            redirect_url,
        }
    }

    /// What the UPDATE leaves: the configuration it states while it leaves the feature
    /// enabled, and nothing at all once the flag leaves it disabled — so a configuration an
    /// earlier write stored is never what a reader reports after an update.
    fn updated(&self) -> ReportedPunchOut {
        let redirect_url =
            (self.stored_flag() && self.states_config).then(|| STATED_REDIRECT_URL.to_string());
        ReportedPunchOut {
            enabled: Some(self.stored_flag()),
            redirect_url,
        }
    }
}

/// Every combination of portal, enable flag, stated configuration and already-stored
/// configuration the rule reads.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for &portal in PORTAL_NAMES {
        for enabled in [Some(true), Some(false), None] {
            for states_config in [false, true] {
                for pre_stored_config in [false, true] {
                    rows.push(Row {
                        case: format!(
                            "{portal}, enabled {enabled:?}, config stated {states_config}, \
                             config already stored {pre_stored_config}"
                        ),
                        portal,
                        enabled,
                        states_config,
                        pre_stored_config,
                    });
                }
            }
        }
    }
    rows
}

/// A stored configuration is reported only while the flag it belongs to leaves Marketplace
/// PunchOut enabled, from the create and from the configuration update alike.
#[tokio::test]
async fn the_punch_out_configuration_is_stored_only_while_the_feature_is_enabled() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in rows().iter().enumerate() {
        let case = &row.case;
        let stated_config = row.states_config.then_some(STATED_REDIRECT_URL);

        // The CREATE path states the portal on its own input. One identity per row, so a
        // conflict never stands in for the rule's answer.
        let created = with_punch_out(
            minimal_create(
                row.portal,
                &format!("create-buyer-{index}"),
                &format!("create-supplier-{index}"),
            ),
            row.enabled,
            stated_config,
        );
        let (create_status, create_body) =
            call(&router, "CreateProcurementPortalPreference", &created).await;

        // The UPDATE path reads the portal off the STORED preference, so it needs one: this
        // baseline create holds a configuration for the rows that ask for one, which it
        // states together with the enabled flag that configuration belongs to.
        let baseline_request = minimal_create(
            row.portal,
            &format!("update-buyer-{index}"),
            &format!("update-supplier-{index}"),
        );
        let baseline_request = if row.pre_stored_config {
            with_punch_out(baseline_request, Some(true), Some(PRE_STORED_REDIRECT_URL))
        } else {
            baseline_request
        };
        let baseline = arn_of(&ok(&router, "CreateProcurementPortalPreference", &baseline_request).await);
        let before = reported(&router, &baseline).await;
        let expected_before = ReportedPunchOut {
            enabled: Some(row.pre_stored_config),
            redirect_url: row
                .pre_stored_config
                .then(|| PRE_STORED_REDIRECT_URL.to_string()),
        };
        assert_eq!(
            before, expected_before,
            "case {case}: the baseline create must report {expected_before:?}, got {before:?}"
        );

        let updated = with_punch_out(put_request(&baseline), row.enabled, stated_config);
        let (put_status, put_body) =
            call(&router, "PutProcurementPortalPreference", &updated).await;

        if row.create_refused() {
            assert_eq!(
                create_status, 400,
                "case {case}: the create expected HTTP 400, got {create_status} {create_body}"
            );
            assert_eq!(
                error_code(&create_body),
                "ValidationException",
                "case {case}: the create expected a ValidationException, got {create_body}"
            );
        } else {
            assert_eq!(
                create_status, 200,
                "case {case}: the create expected HTTP 200, got {create_status} {create_body}"
            );
            let created_report = reported(&router, &arn_of(&create_body)).await;
            let expected_created = row.created();
            assert_eq!(
                created_report, expected_created,
                "case {case}: the create must report {expected_created:?}, got {created_report:?}"
            );
        }

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
            // A refused update wrote nothing, so the baseline still reports what its create
            // stored.
            let after = reported(&router, &baseline).await;
            assert_eq!(
                after, expected_before,
                "case {case}: a refused update writes nothing, so the preference must still \
                 report {expected_before:?}, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            put_status, 200,
            "case {case}: the update expected HTTP 200, got {put_status} {put_body}"
        );

        let updated_report = reported(&router, &arn_of(&put_body)).await;
        let expected_updated = row.updated();
        assert_eq!(
            updated_report, expected_updated,
            "case {case}: the update must report {expected_updated:?}, got {updated_report:?}"
        );

        // A configuration the update took off is GONE, not hidden behind the flag: a later
        // update that enables the feature again has to state a configuration of its own,
        // and one that does not is refused rather than reviving the configuration the
        // disabling update removed.
        if !row.stored_flag() && row.pre_stored_config {
            let reenabled = with_punch_out(put_request(&baseline), Some(true), None);
            let (reenabled_status, reenabled_body) =
                call(&router, "PutProcurementPortalPreference", &reenabled).await;
            assert_eq!(
                reenabled_status, 400,
                "case {case}: re-enabling without stating a configuration expected HTTP 400, \
                 got {reenabled_status} {reenabled_body}"
            );
            let after = reported(&router, &baseline).await;
            let expected_after = ReportedPunchOut {
                enabled: Some(false),
                redirect_url: None,
            };
            assert_eq!(
                after, expected_after,
                "case {case}: the disabling update removed the configuration and the refused \
                 one wrote nothing, so the preference must report {expected_after:?}, got \
                 {after:?}"
            );
        }
    }
}
