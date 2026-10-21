// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice service.
//!
//! The whole service is one resource: a *home region control*, which records the
//! region an account picked as its Migration Hub home region. Every operation of
//! this service reads or writes the two maps below and nothing else.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// One stored home region control — the persisted form of the modelled
/// `HomeRegionControl` shape.
///
/// `requested_time` is kept as a [`SystemTime`] rather than an
/// `aws_smithy_types::DateTime` so the store holds a plain clock reading and the
/// wire type is built where the response is serialized.
#[derive(Debug, Clone)]
pub struct HomeRegionControlRecord {
    /// `"hrc-"` followed by 12 lowercase alphanumerics (the model's `ControlId`
    /// pattern `^hrc-[a-z0-9]{12}$`).
    pub control_id: String,
    /// The region this control set as the home region.
    pub home_region: String,
    /// The target type, always `ACCOUNT` (the model's `TargetType` enum has that
    /// single value).
    pub target_type: String,
    /// The 12-digit id of the account the control was created for.
    pub target_id: Option<String>,
    /// When `CreateHomeRegionControl` was called.
    pub requested_time: SystemTime,
}

impl HomeRegionControlRecord {
    /// This record as the modelled `HomeRegionControl` shape a response carries.
    pub fn to_modeled(&self) -> crate::ops::types::HomeRegionControl {
        crate::ops::types::HomeRegionControl {
            control_id: Some(self.control_id.clone()),
            home_region: Some(self.home_region.clone()),
            target: Some(crate::ops::types::Target {
                r#type: self.target_type.clone(),
                id: self.target_id.clone(),
            }),
            requested_time: Some(::aws_smithy_types::DateTime::from(self.requested_time)),
        }
    }

    /// Whether this record satisfies the `DescribeHomeRegionControls` filters a
    /// caller supplied. An absent filter matches everything, and the filters
    /// combine with AND — the way every AWS list/describe filter set does.
    pub fn matches(
        &self,
        control_id: Option<&str>,
        home_region: Option<&str>,
        target_type: Option<&str>,
        target_id: Option<&str>,
    ) -> bool {
        if control_id.is_some_and(|wanted| wanted != self.control_id) {
            return false;
        }
        if home_region.is_some_and(|wanted| wanted != self.home_region) {
            return false;
        }
        if target_type.is_some_and(|wanted| wanted != self.target_type) {
            return false;
        }
        if target_id.is_some_and(|wanted| Some(wanted) != self.target_id.as_deref()) {
            return false;
        }
        true
    }
}

/// Per-service state for com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState {
    /// Every home region control, keyed by its `ControlId`. The ids minted by
    /// [`Self::allocate_control_id`] sort in creation order, so iterating this
    /// map is a stable order for pagination.
    pub home_region_controls: BTreeMap<String, HomeRegionControlRecord>,
    /// The control that holds each account's home region, keyed by the 12-digit
    /// account id. An account has at most one home region, so this map is what
    /// `GetHomeRegion` reads and what `CreateHomeRegionControl` checks before it
    /// mints a second control for the same account.
    pub home_region_by_account: BTreeMap<String, String>,
    /// Monotonic counter feeding [`Self::allocate_control_id`].
    pub next_control_seq: u64,
}

impl ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState {
    /// Mint an unused `ControlId`.
    ///
    /// The model documents the shape of the id: `"hrc-"` followed by 12
    /// lowercase letters and numbers (`^hrc-[a-z0-9]{12}$`). Lowercase hex of a
    /// monotonic counter is within that alphabet, is unique per call, and sorts
    /// in creation order — the ordering [`Self::home_region_controls`] relies on.
    /// The low 48 bits are used so the rendering is always exactly 12 digits, and
    /// the loop skips an id already in the store (reachable only if the counter
    /// ever wrapped past 2^48 ids).
    pub fn allocate_control_id(&mut self) -> String {
        loop {
            self.next_control_seq = self.next_control_seq.wrapping_add(1);
            let id = format!("hrc-{:012x}", self.next_control_seq & 0xffff_ffff_ffff);
            if !self.home_region_controls.contains_key(&id) {
                return id;
            }
        }
    }

    /// The control holding `account`'s home region, if it has one.
    pub fn control_for_account(&self, account: &str) -> Option<&HomeRegionControlRecord> {
        let control_id = self.home_region_by_account.get(account)?;
        self.home_region_controls.get(control_id)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState>)
}
