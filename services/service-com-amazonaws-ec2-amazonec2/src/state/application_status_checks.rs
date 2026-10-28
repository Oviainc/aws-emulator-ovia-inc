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
//! its state: this surface grows as the sibling modify and delete operations are implemented,
//! and it grows here rather than in the parts of `state.rs` the other resource kinds own.
//! `pub use` keeps every name reachable as `crate::state::<name>`, which is where each
//! handler names it.
//!
//! `CreateApplicationStatusCheck` is what MINTS a check, so the documented defaults it fills
//! an unspecified member with, the published bounds it enforces, the quota it asks about, the
//! bytes a stored check is charged and the comparison a replayed `ClientToken` is judged on
//! all live here, beside the store they are about.

use crate::ops::types::{ApplicationStatusCheckResponseObject, HealthCheckPathResponseObject};
use crate::scaffolded::Scaffolded;

use super::{same_tags, tag_list_bytes, ComAmazonawsEc2Amazonec2State};

/// The id prefix an application status check carries.
///
/// The model publishes no `@pattern` for `ApplicationStatusCheckId` (it is a plain `String`
/// — `smithy_query(query="shape",shape="ApplicationStatusCheckId")`), so the id takes EC2's
/// own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with a prefix naming the resource — the same
/// arrangement `ROUTE_SERVER_ID_PREFIX` and the VPC endpoint connection id use.
///
/// `CreateApplicationStatusCheck` mints every stored check's id with this prefix, through
/// [`ComAmazonawsEc2Amazonec2State::next_id`].
pub const APPLICATION_STATUS_CHECK_ID_PREFIX: &str = "asc";

/// The `TagSpecification$ResourceType` a create may name when it tags the check it creates:
/// the `application-status-check` member of the modelled `ResourceType` catalog
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE: &str = "application-status-check";

/// How many application status checks one account holds before a create is refused: "You can
/// create a maximum of 50 application status checks for each account"
/// (`CreateApplicationStatusCheck`).
///
/// A DOCUMENTED quota rather than a ceiling this emulator chose, so it is the bound a create
/// asks about — and it counts the ACTIVE checks, a deleted one inside its grace period having
/// been given up by the account already.
pub const MAX_APPLICATION_STATUS_CHECKS: usize = 50;

/// The one interval a check may be configured with: "The interval, in seconds, between
/// health checks. Valid value: 60" (`CreateApplicationStatusCheckRequest$Interval`), which is
/// therefore also the interval a request naming none gets ("Interval is 60 seconds").
pub const APPLICATION_STATUS_CHECK_INTERVAL_SECONDS: i32 = 60;

/// The timeout bounds a check may be configured with: "Valid values: 1 to 30"
/// (`CreateApplicationStatusCheckRequest$Timeout`). The member carries no modelled `@range`,
/// so this is the only place those published bounds are enforced.
pub const APPLICATION_STATUS_CHECK_TIMEOUT_RANGE: (i32, i32) = (1, 30);

/// The path a request naming none gets: "You must start the Path with a forward slash (/).
/// Default: /" (`CreateApplicationStatusCheck`).
///
/// There is no constant here for the aggregation default — "it defaults to `included`, which
/// means the check contributes to the instance-level application status" — because that word
/// is the `included` member of `AggregationStatusEnum`, which this module already names once
/// as [`super::AGGREGATION_INCLUDED`].
pub const DEFAULT_APPLICATION_STATUS_CHECK_PATH: &str = "/";

/// The timeout a request naming none gets: "Timeout is 6 seconds"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_TIMEOUT: i32 = 6;

/// The failure threshold a request naming none gets: "FailureThreshold is 2"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_FAILURE_THRESHOLD: i32 = 2;

/// The success threshold a request naming none gets: "SuccessThreshold is 2"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_SUCCESS_THRESHOLD: i32 = 2;

/// The status code matcher a request naming none gets: "StatusCodeMatcher is 200"
/// (`CreateApplicationStatusCheck`), also stated on the member itself ("Default: 200").
pub const DEFAULT_APPLICATION_STATUS_CHECK_STATUS_CODE_MATCHER: &str = "200";

/// The initialization grace period a request naming none gets:
/// "InitializationGracePeriodSeconds is 300 seconds" (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_GRACE_PERIOD_SECONDS: i32 = 300;

/// The longest status code matcher a check may carry: "Maximum length: 64 characters"
/// (`CreateApplicationStatusCheckRequest$StatusCodeMatcher`). The member is a plain `String`
/// with no modelled `@length`, so this is the only place that published bound is enforced.
pub const MAX_STATUS_CODE_MATCHER_CHARS: usize = 64;

/// The longest health check path a check may carry.
///
/// CHOSEN: the member is "The URL path to use for the health check HTTP request"
/// (`CreateApplicationStatusCheckRequest$Path`), a plain `String` the model bounds nowhere and
/// AWS publishes no length for, and the store keeps whatever it is given for the process's
/// lifetime — so the caller-chosen bytes one check can hold have to be finite. It is well
/// above any URL path a health check uses, so no request a real account would make is refused
/// for a reason this emulator cannot have.
pub const MAX_HEALTH_CHECK_PATH_CHARS: usize = 1_024;

/// How many health check paths one check may carry, and how many destinations each may name.
///
/// CHOSEN for the same reason as [`MAX_HEALTH_CHECK_PATH_CHARS`]: `HealthCheckPathRequestList`
/// and `HealthCheckPathDestinationRequestSet` carry no modelled `@length`, so the number of
/// subnet ids one create can retain has to be bounded somewhere, and the product of the two is
/// what bounds the whole `HealthCheckPaths` member.
pub const MAX_HEALTH_CHECK_PATHS: usize = 10;

/// How many destinations one health check path may name — see [`MAX_HEALTH_CHECK_PATHS`].
pub const MAX_HEALTH_CHECK_PATH_DESTINATIONS: usize = 10;

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

/// How many ACTIVE application status checks the account holds right now — what a create
/// compares against [`MAX_APPLICATION_STATUS_CHECKS`].
///
/// Counted off [`application_status_check_is_active`], so the quota and the describe agree
/// about which checks the account still holds: one inside its deletion grace period is
/// reported only when a caller asks for it and does not stand between the account and its
/// next create.
pub fn active_application_status_check_count(store: &ComAmazonawsEc2Amazonec2State) -> usize {
    store
        .application_status_checks
        .values()
        .filter(|check| application_status_check_is_active(check))
        .count()
}

/// Refuse a create that would take the account past the documented quota: HTTP 400
/// [`super::RESOURCE_LIMIT_EXCEEDED`], the code this service answers for a published ceiling
/// with no code of its own.
pub fn application_status_check_limit_exceeded<E>(held: usize) -> Scaffolded<E> {
    super::resource_limit_exceeded(format!(
        "You have reached the limit of {MAX_APPLICATION_STATUS_CHECKS} application status \
         checks for this account, which already holds {held}"
    ))
}

/// Whether `matcher` is the form `StatusCodeMatcher` documents: "a comma-separated list of
/// individual status codes or ranges, for example, 200,202,300-399. For a range, the first
/// value must be less than the second value"
/// (`CreateApplicationStatusCheckRequest$StatusCodeMatcher`).
///
/// An HTTP status code is three digits, so each entry is either one three-digit code or two
/// separated by a hyphen, the first below the second. The LENGTH bound the same member
/// publishes is [`MAX_STATUS_CODE_MATCHER_CHARS`], asked separately so a matcher that is too
/// long is refused for being too long rather than for its form.
pub fn status_code_matcher_is_well_formed(matcher: &str) -> bool {
    !matcher.is_empty()
        && matcher.split(',').all(|entry| match entry.split_once('-') {
            Some((low, high)) => match (http_status_code(low), http_status_code(high)) {
                (Some(low), Some(high)) => low < high,
                _ => false,
            },
            None => http_status_code(entry).is_some(),
        })
}

/// One entry of a [`status_code_matcher_is_well_formed`] matcher as the status code it names,
/// or `None` when it is not one: an HTTP status code is exactly three decimal digits.
fn http_status_code(text: &str) -> Option<u16> {
    (text.len() == 3 && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The caller-chosen bytes one stored application status check holds: its tags, its path, its
/// status code matcher, the subnet and security group ids of its health check paths, and the
/// `ClientToken` the token map keeps for it. Everything else on the record — the id, the
/// ports, the thresholds, the instants — is minted by this module or bounded by the model, so
/// it is fixed-size.
///
/// The ONE definition of that charge, the way [`super::route_server_payload_bytes`] is for a
/// route server: a create asks it before the write and charges exactly it, and a removal path
/// must give back the same number.
pub fn application_status_check_payload_bytes(
    check: &ApplicationStatusCheckResponseObject,
    client_token: Option<&str>,
) -> usize {
    let paths: usize = check
        .health_check_paths
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(health_check_path_bytes)
        .sum();
    tag_list_bytes(check.tags.as_deref().unwrap_or_default())
        .saturating_add(check.path.as_deref().map_or(0, str::len))
        .saturating_add(check.status_code_matcher.as_deref().map_or(0, str::len))
        .saturating_add(paths)
        .saturating_add(client_token.map_or(0, str::len))
}

/// The caller-chosen bytes one health check path holds: the ids its source and each of its
/// destinations name.
fn health_check_path_bytes(path: &HealthCheckPathResponseObject) -> usize {
    let source = path.source.as_ref().map_or(0, |source| {
        source
            .subnet_id
            .as_deref()
            .map_or(0, str::len)
            .saturating_add(source.security_group_id.as_deref().map_or(0, str::len))
    });
    path.destinations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .fold(source, |bytes, destination| {
            bytes
                .saturating_add(destination.subnet_id.as_deref().map_or(0, str::len))
                .saturating_add(
                    destination
                        .security_group_id
                        .as_deref()
                        .map_or(0, str::len),
                )
        })
}

/// Whether two application status checks are configured the SAME way — every member a create
/// takes from the request, and nothing the service decides for itself (the id and the
/// instants).
///
/// What an idempotency replay is judged on: "If you retry a request with the same token, the
/// service ignores the request but does not return an error"
/// (`CreateApplicationStatusCheckRequest$ClientToken`) holds for a retry of the SAME request,
/// so a token reused for a different configuration is the mismatch EC2 refuses instead. The
/// modelled shapes carry no `PartialEq`, which is why the comparison is spelled out here once
/// rather than in each operation that needs it.
pub fn same_application_status_check_configuration(
    stored: &ApplicationStatusCheckResponseObject,
    asked: &ApplicationStatusCheckResponseObject,
) -> bool {
    stored.aggregation == asked.aggregation
        && stored.protocol == asked.protocol
        && stored.port == asked.port
        && stored.path == asked.path
        && stored.device_index == asked.device_index
        && stored.ip_version == asked.ip_version
        && stored.ip_scope == asked.ip_scope
        && stored.interval == asked.interval
        && stored.timeout == asked.timeout
        && stored.failure_threshold == asked.failure_threshold
        && stored.success_threshold == asked.success_threshold
        && stored.status_code_matcher == asked.status_code_matcher
        && stored.initialization_grace_period_seconds == asked.initialization_grace_period_seconds
        && same_health_check_paths(
            stored.health_check_paths.as_deref().unwrap_or_default(),
            asked.health_check_paths.as_deref().unwrap_or_default(),
        )
        && same_tags(
            stored.tags.as_deref().unwrap_or_default(),
            asked.tags.as_deref().unwrap_or_default(),
        )
}

/// Whether two health check path lists name the same paths in the same order.
///
/// ORDER matters, unlike a tag list's: a path list is a sequence the caller gave, not a set
/// keyed by anything, so nothing makes two orderings the same request.
fn same_health_check_paths(
    stored: &[HealthCheckPathResponseObject],
    asked: &[HealthCheckPathResponseObject],
) -> bool {
    stored.len() == asked.len()
        && stored.iter().zip(asked).all(|(stored, asked)| {
            let same_source = match (stored.source.as_ref(), asked.source.as_ref()) {
                (None, None) => true,
                (Some(stored), Some(asked)) => {
                    stored.subnet_id == asked.subnet_id
                        && stored.security_group_id == asked.security_group_id
                }
                _ => false,
            };
            let stored_destinations = stored.destinations.as_deref().unwrap_or_default();
            let asked_destinations = asked.destinations.as_deref().unwrap_or_default();
            same_source
                && stored_destinations.len() == asked_destinations.len()
                && stored_destinations
                    .iter()
                    .zip(asked_destinations)
                    .all(|(stored, asked)| {
                        stored.subnet_id == asked.subnet_id
                            && stored.security_group_id == asked.security_group_id
                    })
        })
}
