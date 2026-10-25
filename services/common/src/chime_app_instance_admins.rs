// SPDX-License-Identifier: CC0-1.0

//! The APP-INSTANCE ADMIN directory: which identity administers an Amazon Chime SDK app
//! instance.
//!
//! An `AppInstanceAdmin` is a resource of ONE service — Chime SDK Identity promotes an
//! identity to it — while the role it grants is read by ANOTHER: Chime SDK Messaging
//! reserves actions for an administrator of the app instance a channel belongs to (listing
//! an account's private channels, deleting a channel message). The messaging service
//! cannot measure that role, because the promotion was given to the identity service, so
//! the identity service RECORDS it here and the messaging service reads it.
//!
//! One directory under ONE shared type, because the engine keys state by type: a copy
//! inside either service's own state struct would be invisible to the other, and the two
//! would disagree about who administers an app instance.
//!
//! Nothing here names a wire shape: the directory holds the app instance ARN, the
//! administering identity's ARN and the instant the promotion was recorded, and each
//! reader projects those onto the shapes ITS model declares.
//!
//! The directory is a record of what this emulator PROMOTED, so it is partial by
//! construction: an identity it holds no record of simply is not an administrator here.
//! It is also BOUNDED — see [`MAX_REGISTERED_ADMINS`] — so a caller looping on promote
//! cannot grow it without end; the oldest registration is dropped first, which keeps the
//! promotions a live workflow just made readable.

use std::collections::{BTreeMap, VecDeque};

/// How many promotions the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so without a bound a caller promoting identities in a loop would grow
/// it without end. It is set far above any ordinary workflow, and past it the OLDEST
/// registration is dropped — never a newer one — so an identity a running workflow just
/// promoted is always still an administrator.
pub const MAX_REGISTERED_ADMINS: usize = 10_000;

/// One recorded promotion: an identity that administers an app instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInstanceAdminRecord {
    /// The `AppInstanceArn` the identity administers.
    pub app_instance_arn: String,
    /// The administering identity's ARN (an `AppInstanceUser` or `AppInstanceBot`).
    pub admin_arn: String,
    /// When the promotion was recorded, as epoch milliseconds — the resolution the
    /// restJson1 timestamps of both services carry.
    pub created_millis: i64,
}

/// The directory itself: every recorded promotion, keyed by app instance and identity.
///
/// `BTreeMap` (not `HashMap`) so a reader that lists an app instance's administrators
/// walks them in a stable order, which is what makes a paginated answer resumable.
#[derive(Debug, Default)]
pub struct AppInstanceAdminDirectory {
    admins: BTreeMap<(String, String), AppInstanceAdminRecord>,
    /// The order the entries were written in, so the oldest can be dropped first.
    order: VecDeque<(String, String)>,
}

impl AppInstanceAdminDirectory {
    /// Record that `admin_arn` administers `app_instance_arn`, and answer the record the
    /// directory now holds for it.
    ///
    /// Promotion is a SET: recording one the directory already holds keeps the FIRST
    /// record, so the instant it reports does not move every time a caller repeats the
    /// request. Past [`MAX_REGISTERED_ADMINS`] the oldest registration is dropped.
    pub fn promote(
        &mut self,
        app_instance_arn: &str,
        admin_arn: &str,
        created_millis: i64,
    ) -> AppInstanceAdminRecord {
        let key = (app_instance_arn.to_string(), admin_arn.to_string());
        if let Some(existing) = self.admins.get(&key) {
            return existing.clone();
        }
        let record = AppInstanceAdminRecord {
            app_instance_arn: app_instance_arn.to_string(),
            admin_arn: admin_arn.to_string(),
            created_millis,
        };
        self.admins.insert(key.clone(), record.clone());
        self.order.push_back(key);
        while self.order.len() > MAX_REGISTERED_ADMINS {
            if let Some(oldest) = self.order.pop_front() {
                self.admins.remove(&oldest);
            }
        }
        record
    }

    /// The recorded promotion of `admin_arn` over `app_instance_arn`, if the directory
    /// holds one.
    pub fn record(
        &self,
        app_instance_arn: &str,
        admin_arn: &str,
    ) -> Option<&AppInstanceAdminRecord> {
        self.admins
            .get(&(app_instance_arn.to_string(), admin_arn.to_string()))
    }

    /// Whether `admin_arn` administers `app_instance_arn`.
    pub fn administers(&self, app_instance_arn: &str, admin_arn: &str) -> bool {
        self.record(app_instance_arn, admin_arn).is_some()
    }

    /// Every recorded administrator of `app_instance_arn`, in a stable order.
    pub fn admins_of(&self, app_instance_arn: &str) -> Vec<&AppInstanceAdminRecord> {
        self.admins
            .values()
            .filter(|record| record.app_instance_arn == app_instance_arn)
            .collect()
    }

    /// Drop `admin_arn`'s promotion over `app_instance_arn`, answering whether the
    /// directory held one.
    pub fn demote(&mut self, app_instance_arn: &str, admin_arn: &str) -> bool {
        let key = (app_instance_arn.to_string(), admin_arn.to_string());
        self.order.retain(|held| *held != key);
        self.admins.remove(&key).is_some()
    }
}
