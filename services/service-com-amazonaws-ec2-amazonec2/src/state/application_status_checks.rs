// SPDX-License-Identifier: CC0-1.0

//! The application status CHECKS this service stores, and what reads one.
//!
//! The CONFIGURATION of each application status check the account holds — protocol, port,
//! path, thresholds, health check paths and tags — lives in
//! [`ComAmazonawsEc2Amazonec2State::application_status_checks`], keyed by
//! `ApplicationStatusCheckId`, as the modelled [`ApplicationStatusCheckResponseObject`]
//! `DescribeApplicationStatusChecks` reports, so every member a writer sets round-trips
//! without a field-by-field copy that can silently drop one. That is a different store from
//! [`ComAmazonawsEc2Amazonec2State::application_status`], which holds what each check
//! REPORTED about one instance: one configuration, many per-instance results.
//!
//! A check inside its deletion grace period is the stored configuration with its
//! `DeletionTime` set — "recently deleted application status checks that remain available
//! during the deletion grace period"
//! (`DescribeApplicationStatusChecksRequest$IncludeAll`) — so whether a check is active is
//! read off that member ([`application_status_check_is_active`]) rather than from a second
//! map of deleted checks.
//!
//! A SUBMODULE of [`crate::state`], the way CloudFront's function slice is a submodule of
//! its state: this surface grows as the sibling create, modify and delete operations are
//! implemented, and it grows here rather than in the parts of `state.rs` the other resource
//! kinds own. `pub use` keeps every name reachable as `crate::state::<name>`, which is where
//! each handler names it.

use crate::ops::types::ApplicationStatusCheckResponseObject;
use crate::scaffolded::Scaffolded;

use super::ComAmazonawsEc2Amazonec2State;

/// The id prefix an application status check carries.
///
/// The model publishes no `@pattern` for `ApplicationStatusCheckId` (it is a plain `String`
/// — `smithy_query(query="shape",shape="ApplicationStatusCheckId")`), so the id takes EC2's
/// own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with a prefix naming the resource — the same
/// arrangement `ROUTE_SERVER_ID_PREFIX` and the VPC endpoint connection id use.
///
/// Nothing MINTS a check yet: `CreateApplicationStatusCheck` is still a scaffold stub with
/// its own issue, so this prefix is the form the writer to come will mint with, and today it
/// is exercised only by the tests that seed the store.
pub const APPLICATION_STATUS_CHECK_ID_PREFIX: &str = "asc";

/// `InvalidApplicationStatusCheckId.NotFound` (HTTP 400): the request named an application
/// status check id this account holds no check for. EC2 spells a per-resource lookup failure
/// `Invalid<Resource>Id.NotFound` (`InvalidRouteServerId.NotFound`,
/// `InvalidLaunchTemplateId.NotFound`).
pub const INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND: &str =
    "InvalidApplicationStatusCheckId.NotFound";

/// Refuse a request that named an application status check id no check of the account has:
/// HTTP 400 [`INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND`].
pub fn invalid_application_status_check_id_not_found<E>(
    application_status_check_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND,
        400,
        format!("The application status check ID '{application_status_check_id}' does not exist"),
    )
}

/// Refuse an application status check id the request named that this service cannot resolve.
///
/// Unlike the route server and launch template ids, there is no `Malformed` half: the model
/// publishes no pattern for `ApplicationStatusCheckId`, so nothing distinguishes "not an id
/// of this kind" from "no such check" without inventing a pattern AWS does not publish.
/// Every unresolvable id is therefore reported as the one thing that is known about it — the
/// account holds no check for it.
pub fn require_known_application_status_check<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    application_status_check_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !store
        .application_status_checks
        .contains_key(application_status_check_id)
    {
        return Err(invalid_application_status_check_id_not_found(
            application_status_check_id,
        ));
    }
    Ok(())
}

/// Whether `check` is ACTIVE — outside the deletion grace period a deleted check spends
/// still visible.
///
/// "If you omit this parameter or set it to false, the response includes only active checks"
/// (`DescribeApplicationStatusChecksRequest$IncludeAll`), and a deleted check is the one that
/// carries "the date and time when the application status check was deleted"
/// (`ApplicationStatusCheckResponseObject$DeletionTime`) — so that member is what tells the
/// two apart, in one place, for every operation that has to.
pub fn application_status_check_is_active(check: &ApplicationStatusCheckResponseObject) -> bool {
    check.deletion_time.is_none()
}
