// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdashboards-awsbcmdashboardsservice service.
//!
//! The dashboard records every dashboard operation reads and writes live here, so
//! `CreateDashboard`, `GetDashboard`, `ListDashboards`, `UpdateDashboard` and
//! `DeleteDashboard` all see one copy of each resource. The helpers below (ARN and
//! widget-id minting, the `DashboardReference` projection, the pagination cursor
//! codec) are shared by those handlers rather than duplicated per operation.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{DashboardReference, ResourceTag, Widget};

/// The only `DashboardType` the model declares (`DASHBOARD_TYPE_VALUES`), and the
/// type every dashboard a client creates through this API carries.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";

/// The prefix a `ListDashboards` pagination cursor carries, so a token this
/// service did not mint is rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";

/// The most dashboards this emulator stores for the account before `CreateDashboard`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's dashboard quota is not stated in the model, so this bound is
/// CHOSEN, not cited: it is set far above any plausible client's dashboard count, so
/// normal use never meets it, while the store a caller can grow stays bounded.
pub const MAX_DASHBOARDS: usize = 1_000;

/// One stored dashboard.
///
/// `widgets` holds the modelled [`Widget`] shape verbatim — including the widget
/// ids this service minted and the `width`/`height`/`horizontalOffset` values the
/// model's defaults filled in — so `GetDashboard` reads back exactly what was
/// stored rather than re-deriving anything from the request.
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// `DashboardType` as it goes on the wire; always [`DASHBOARD_TYPE_CUSTOM`]
    /// for a dashboard created through `CreateDashboard`.
    pub dashboard_type: String,
    pub widgets: Vec<Widget>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub resource_tags: Vec<ResourceTag>,
}

impl Dashboard {
    /// This dashboard projected onto the `DashboardReference` shape `ListDashboards`
    /// returns: the stored record's own fields, never request input.
    pub fn reference(&self) -> DashboardReference {
        DashboardReference {
            arn: self.arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.dashboard_type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// Per-service state for com-amazonaws-bcmdashboards-awsbcmdashboardsservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Every stored dashboard, keyed by its ARN. A `BTreeMap` so `ListDashboards`
    /// walks the dashboards in one stable order and a cursor taken on one page
    /// resumes at the right place on the next.
    pub dashboards: BTreeMap<String, Dashboard>,
    /// Monotonic counter behind every minted dashboard resource id, so two
    /// `CreateDashboard` calls with the same `name` get different ARNs.
    pub next_dashboard_seq: u64,
    /// Monotonic counter behind every minted widget id.
    pub next_widget_seq: u64,
}

impl ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Mint a fresh, unique dashboard ARN.
    ///
    /// The resource id is 32 lowercase hex characters, which satisfies the
    /// `DashboardArn` pattern's `[-a-z0-9]+` resource segment; the account segment
    /// comes from the emulator's canonical account id via `engine::emulator_arn`.
    /// `bcm-dashboards` ARNs carry no region (the model's pattern has an empty
    /// region segment), so the region passed is empty.
    pub fn mint_dashboard_arn(&mut self) -> String {
        self.next_dashboard_seq = self.next_dashboard_seq.wrapping_add(1);
        let resource_id = format!("{:032x}", self.next_dashboard_seq);
        ::engine::emulator_arn("bcm-dashboards", "", &format!("dashboard/{resource_id}"))
    }

    /// Mint a fresh widget id: 32 lowercase hex characters, as `WidgetId`'s
    /// `^[0-9a-f]{32}$` pattern requires.
    pub fn mint_widget_id(&mut self) -> String {
        self.next_widget_seq = self.next_widget_seq.wrapping_add(1);
        format!("{:032x}", self.next_widget_seq)
    }

    /// The widgets to store for a create or update: the supplied widgets with an
    /// id minted for every one that arrived without one. `width`, `height` and
    /// `horizontalOffset` already carry the model's declared defaults (4, 7, 0),
    /// which the framework fills in before a handler runs.
    pub fn adopt_widgets(&mut self, widgets: Vec<Widget>) -> Vec<Widget> {
        widgets
            .into_iter()
            .map(|mut widget| {
                if widget.id.as_deref().is_none_or(str::is_empty) {
                    widget.id = Some(self.mint_widget_id());
                }
                widget
            })
            .collect()
    }
}

/// Encode the ARN a `ListDashboards` page resumes at as an opaque `nextToken`.
pub fn encode_page_token(arn: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(arn))
}

/// The ARN a `nextToken` resumes at, or `None` when the token is not one this
/// service minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    // A cursor this service minted always names a dashboard ARN. Checked against the
    // fixed segments of `DashboardArn`'s pattern
    // (`^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/...`), so a token that
    // decodes to anything else is rejected rather than read as a resume point past
    // every stored dashboard.
    let names_a_dashboard = arn.starts_with("arn:aws")
        && arn.contains(":bcm-dashboards::")
        && arn.contains(":dashboard/");
    names_a_dashboard.then_some(arn)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>)
}
