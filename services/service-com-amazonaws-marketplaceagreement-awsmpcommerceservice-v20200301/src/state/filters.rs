// SPDX-License-Identifier: CC0-1.0

//! Selecting with filters: the filter members this service publishes, how a
//! request's filters are validated, and the scopes a listing or a search
//! paginates over.

use super::*;

/// Why a `SearchAgreements` filter was refused: the `ValidationException`
/// `reason` to report, and the dotted request path of the member it is about —
/// which element of `filters` carried it, and which member of that element.
#[derive(Debug)]
pub struct FilterRefusal {
    pub reason: &'static str,
    pub path: String,
}

/// The dotted request path of one member of the `index`-th element of the
/// request's `filters` list — `filters.1.member.name` for the FIRST filter's
/// `name`.
///
/// Spelled through the framework's own path rule
/// ([`::aws_smithy_schema_server::constraints::path`]), so a path this service
/// builds for a rule the model cannot express reads exactly like one the
/// boundary validator builds for a modelled constraint: the list member, the
/// element's 1-based position, the step into the element, then the member of it
/// that failed.
pub(super) fn filter_member_path(index: usize, member: &str) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member("filters"),
        path::element(index + 1),
        path::member(member),
    ])
}

/// The dotted request path of the single value one filter carries —
/// `filters.1.member.values.1.member`. `FilterValueList` is modelled
/// `@length(min: 1, max: 1)`, so the offending value is always that list's
/// first element.
pub(super) fn filter_value_path(index: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[filter_member_path(index, "values"), path::element(1)])
}

/// The instant a filter value names, parsed as the RFC-3339 timestamp the
/// awsJson protocols carry a `Timestamp` as.
pub(super) fn filter_instant(value: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// `<byte length>:<value>`, which no value can forge: the part every page-token
/// scope is built from.
pub(super) fn counted(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// The scope part one bound of a time window is: the epoch seconds the call
/// named, to their full precision, and the empty part when the call left that
/// bound out — so a window a caller narrows by any amount, and an open end
/// closed at any instant, never render alike.
///
/// The one spelling of that rule: every page-token scope of this module whose
/// query carries such a window renders BOTH of its bounds through this
/// ([`invoice_line_item_scope`] and [`billing_adjustment_scope`]), so the two
/// never drift apart. The part is length-prefixed by [`query_scope`] like every
/// other, so no rendering of a bound can imitate the delimiters.
pub(super) fn scope_instant(bound: Option<f64>) -> String {
    bound.map(|secs| format!("{secs:?}")).unwrap_or_default()
}

/// Seconds since the epoch, for comparing a stored instant with a filter's or
/// with a `createdAfter`/`createdBefore` bound.
pub fn epoch_secs(instant: SystemTime) -> f64 {
    ::aws_smithy_types::DateTime::from(instant).as_secs_f64()
}

/// Whether `agreement` matches the one filter `name`/`value` names.
///
/// The names are the ones `SearchAgreementsInput$filters` documents
/// ([`SEARCH_FILTER_NAMES`]); each is matched against the member of the stored
/// agreement it names. A filter on something the emulator holds no value for
/// matches nothing — it does not match everything.
///
/// The filter itself is refused by [`validate_filters`] before any agreement is
/// scanned, so a name or value this cannot interpret answers `false` here
/// rather than repeating that refusal.
pub(super) fn matches_filter(agreement: &Agreement, name: &str, value: &str) -> bool {
    let before =
        |stored: Option<SystemTime>| match (stored.map(epoch_secs), filter_instant(value)) {
            (Some(stored), Some(bound)) => stored < bound,
            _ => false,
        };
    let after = |stored: Option<SystemTime>| match (stored.map(epoch_secs), filter_instant(value))
    {
        (Some(stored), Some(bound)) => stored > bound,
        _ => false,
    };
    // The two end-time-behaviour filters select on the members the summary
    // reports, so they read the same behaviour it does.
    let behavior = agreement.end_time_behavior_at_end_date();

    match name {
        "ResourceIdentifier" => agreement
            .proposal_summary
            .resources
            .iter()
            .flatten()
            .any(|resource| resource.id.as_deref() == Some(value)),
        "ResourceType" => agreement
            .proposal_summary
            .resources
            .iter()
            .flatten()
            .any(|resource| resource.r#type.as_deref() == Some(value)),
        // "The party type of the caller": the agreement matches when the
        // caller's own account is the party the filter names.
        "PartyType" => caller_is_party(agreement, value),
        "AcceptorAccountId" => agreement.acceptor.account_id.as_deref() == Some(value),
        "OfferId" => agreement.proposal_summary.offer_id.as_deref() == Some(value),
        "OfferSetId" => agreement.proposal_summary.offer_set_id.as_deref() == Some(value),
        "Status" => agreement.status == value,
        "AgreementType" => agreement.agreement_type == value,
        "InitialAgreementId" => agreement.initial_agreement_id == value,
        "BeforeEndTime" => before(agreement.end_time),
        "AfterEndTime" => after(agreement.end_time),
        "BeforeStartTime" => before(Some(agreement.start_time)),
        "AfterStartTime" => after(Some(agreement.start_time)),
        "BeforeLastUpdateTime" => before(Some(agreement.updated_at)),
        "AfterLastUpdateTime" => after(Some(agreement.updated_at)),
        "EndTimeBehaviorType" => behavior.is_some_and(|b| b.r#type == value),
        "EndTimeBehaviorReasonCode" => {
            behavior.is_some_and(|b| b.reason_code.as_deref() == Some(value))
        }
        // "A License Manager licence granted by the agreement": the licence of
        // one of the entitlements accepting the agreement provisioned.
        "LicenseArn" => agreement
            .entitlements
            .iter()
            .any(|entitlement| entitlement.license_arn == value),
        _ => false,
    }
}

/// The name and the single value one filter carries.
///
/// `FilterValueList` is modelled `@length(min: 1, max: 1)`, so a filter that
/// carries values carries exactly one; both members are optional, and a filter
/// missing either names nothing to select on.
pub(super) fn filter_parts(index: usize, filter: &Filter) -> Result<(&str, &str), FilterRefusal> {
    let name = filter.name.as_deref().ok_or_else(|| FilterRefusal {
        reason: "MISSING_FILTER_NAME",
        path: filter_member_path(index, "name"),
    })?;
    let value = filter
        .values
        .as_ref()
        .and_then(|values| values.first())
        .ok_or_else(|| FilterRefusal {
            reason: "MISSING_FILTER_VALUES",
            path: filter_member_path(index, "values"),
        })?;
    Ok((name, value))
}

/// Refuse a filter list this service cannot interpret, BEFORE any agreement is
/// scanned — so a bad filter is refused the same way whatever the store holds,
/// including when it holds nothing.
/// Each refusal names the element that carried the offending member and the
/// member of it that failed, so a caller with several filters can tell WHICH one
/// was rejected ([`filter_member_path`]).
pub fn validate_filters(filters: &[Filter]) -> Result<(), FilterRefusal> {
    for (index, filter) in filters.iter().enumerate() {
        let (name, value) = filter_parts(index, filter)?;
        if !SEARCH_FILTER_NAMES.contains(&name) {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_NAME",
                path: filter_member_path(index, "name"),
            });
        }
        if let Some((values, reason)) = published_filter_values(name) {
            if !values.contains(&value) {
                return Err(FilterRefusal {
                    reason,
                    path: filter_value_path(index),
                });
            }
        }
        // Every `Before…`/`After…` filter selects on an instant, so its value
        // has to be one.
        if (name.starts_with("Before") || name.starts_with("After"))
            && filter_instant(value).is_none()
        {
            return Err(FilterRefusal {
                reason: "INVALID_FILTER_VALUES",
                path: filter_value_path(index),
            });
        }
    }
    Ok(())
}

/// The closed set of values one filter's value admits, and the
/// `ValidationException` reason a value outside it is refused with — or `None`
/// for a filter whose values are open (an account id, an identifier, an ARN, an
/// instant, a product type whose list `SearchAgreementsInput$filters` leaves
/// open).
///
/// A filter selects on an attribute the model gives a closed value set for
/// (`PartyType`, `AgreementType`, and the enums `AgreementStatus`,
/// `EndTimeBehaviorType` and `EndTimeBehaviorReasonCode`), so a value outside
/// that set names nothing this service could ever hold. The reason is the one
/// `ValidationExceptionReason` models for THAT attribute where it models one,
/// and the generic `INVALID_FILTER_VALUES` otherwise.
pub(super) fn published_filter_values(name: &str) -> Option<(&'static [&'static str], &'static str)> {
    match name {
        "PartyType" => Some((PARTY_TYPE_VALUES, "INVALID_PARTY_TYPE")),
        "AgreementType" => Some((AGREEMENT_TYPE_VALUES, "INVALID_AGREEMENT_TYPE")),
        "Status" => Some((AGREEMENT_STATUS_VALUES, "INVALID_STATUS")),
        "EndTimeBehaviorType" => Some((END_TIME_BEHAVIOR_TYPE_VALUES, "INVALID_FILTER_VALUES")),
        "EndTimeBehaviorReasonCode" => Some((
            END_TIME_BEHAVIOR_REASON_CODE_VALUES,
            "INVALID_FILTER_VALUES",
        )),
        _ => None,
    }
}

/// The party type a search is scoped to: the value of its `PartyType` filter,
/// or `None` when it names none.
///
/// `Sort$sortBy` restricts two of its attributes to one party type, so the
/// attribute a search may sort on is decided by this scope rather than by the
/// agreements the search happens to select. Call [`validate_filters`] first: a
/// `PartyType` value it would refuse is not a scope.
pub fn party_type_scope(filters: &[Filter]) -> Option<&str> {
    filters
        .iter()
        .enumerate()
        .find_map(|(index, filter)| match filter_parts(index, filter) {
            Ok(("PartyType", value)) => Some(value),
            _ => None,
        })
}

/// The filter names a search carries, in the order it listed them.
///
/// A filter [`validate_filters`] would refuse carries no name here: call it
/// first, so a list this reads is one whose every element names a documented
/// filter.
pub(super) fn filter_names(filters: &[Filter]) -> Vec<&str> {
    filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| filter_parts(index, filter).ok())
        .map(|(name, _)| name)
        .collect()
}

/// Refuse a `SearchAgreements` filter LIST whose names, each documented on its
/// own, are not a combination the service searches on — BEFORE any agreement is
/// scanned, so the answer does not depend on what the store holds.
///
/// `SearchAgreementsInput$filters` states two rules over the list as a whole:
/// [`REQUIRED_SEARCH_FILTER_NAME`] is required of every search, and a search
/// scoped to [`PARTY_TYPE_ACCEPTOR`] may add only the names one of the
/// combinations that member lists adds ([`ACCEPTOR_FILTER_COMBINATIONS`]). A
/// search scoped to [`PARTY_TYPE_PROPOSER`], or to no party type at all, is
/// under the first rule only: "a proposer can use any combination of the
/// preceding filters along with `AgreementType`".
///
/// Neither rule is about one member being wrong, so both are stated in the
/// combination envelope ([`unsupported_filter_combination`]) naming the list.
/// Call [`validate_filters`] first: a name it would refuse is not part of a
/// combination.
pub fn filter_combination_refusal(filters: &[Filter]) -> Option<ValidationException> {
    let names = filter_names(filters);
    if !names.contains(&REQUIRED_SEARCH_FILTER_NAME) {
        return Some(unsupported_filter_combination(FILTERS_PATH));
    }
    if party_type_scope(filters) != Some(PARTY_TYPE_ACCEPTOR) {
        return None;
    }
    let mut added: Vec<&str> = names
        .into_iter()
        .filter(|name| {
            *name != REQUIRED_SEARCH_FILTER_NAME
                && *name != PARTY_TYPE_FILTER_NAME
                && !END_TIME_FILTER_NAMES.contains(name)
        })
        .collect();
    added.sort_unstable();
    added.dedup();
    if ACCEPTOR_FILTER_COMBINATIONS.contains(&added.as_slice()) {
        None
    } else {
        Some(unsupported_filter_combination(FILTERS_PATH))
    }
}

/// The dotted request path of the filter list itself, which is the member a rule
/// over the WHOLE list is about — no single element of it is the wrong one.
pub(super) const FILTERS_PATH: &str = "filters";

/// Every bound the filter list states through the filter `name`, as the position
/// of the filter that carried it and the instant its value names.
///
/// A list may carry the same bound more than once — the filters are an AND, so
/// each occurrence bounds the same stored instant. Call [`validate_filters`]
/// first: a value it would refuse names no instant and is skipped here.
pub(super) fn filter_bounds(filters: &[Filter], name: &str) -> Vec<(usize, f64)> {
    filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| match filter_parts(index, filter) {
            Ok((filter_name, value)) if filter_name == name => {
                filter_instant(value).map(|instant| (index, instant))
            }
            _ => None,
        })
        .collect()
}

/// Refuse a filter LIST that bounds one stored instant from BOTH sides with its
/// lower bound at or after its upper bound, BEFORE any agreement is scanned.
///
/// A search states such a window as a pair of filter names rather than as a pair
/// of input members ([`FILTER_TIME_WINDOW_NAMES`]), but it is the same pair of
/// exclusive bounds on one instant that [`time_window_refusal`] owns the rule
/// for, so it is answered by that rule — naming the two filter values that
/// bounded the period. The filters are an AND, so a list naming a bound twice
/// states a malformed period as soon as ANY of its lower bounds is at or after
/// any of its upper ones.
///
/// Nothing here reads the store, so the answer is the same whatever it holds.
/// Call [`validate_filters`] first: every bound this compares is a value that
/// parsed as an instant.
pub fn filter_time_window_refusal(filters: &[Filter]) -> Option<ValidationException> {
    FILTER_TIME_WINDOW_NAMES
        .iter()
        .find_map(|&(after_name, before_name)| {
            let uppers = filter_bounds(filters, before_name);
            filter_bounds(filters, after_name)
                .iter()
                .find_map(|&(after_index, after)| {
                    uppers.iter().find_map(|&(before_index, before)| {
                        time_window_refusal(
                            Some(after),
                            Some(before),
                            &filter_value_path(after_index),
                            &filter_value_path(before_index),
                        )
                    })
                })
        })
}

/// Whether `agreement` matches every filter the search named.
///
/// Several filters are an AND, which is what makes the documented combinations
/// (`AgreementType` + `Status`, …) select. Call [`validate_filters`] first: a
/// filter it would refuse matches nothing here.
pub fn matches_filters(agreement: &Agreement, filters: &[Filter]) -> bool {
    filters.iter().enumerate().all(|(index, filter)| {
        filter_parts(index, filter).is_ok_and(|(name, value)| matches_filter(agreement, name, value))
    })
}

/// The concatenation of `parts`, each length-prefixed by [`counted`] so no value
/// a caller supplies can imitate the delimiters and make two different queries
/// hash alike.
///
/// The one spelling of that rule: every page-token scope in this module is built
/// from it, through [`listing_scope`].
pub fn query_scope(parts: &[&str]) -> String {
    parts.iter().copied().map(counted).collect()
}

/// The scope a page token belongs to: the OPERATION that minted it, then every
/// parameter of the call that selects what the page indexes into.
///
/// `operation` is the minting operation's name (`Self::NAME` in a handler). It
/// is part of the scope because two operations paginate two different sequences
/// even when they take the same selectors: without it, two listings whose
/// selector parts coincide would hash alike, and a token minted by one would be
/// accepted by the other as a raw offset into a sequence it never indexed.
///
/// The one spelling of that rule: every page-token scope in this module —
/// [`search_scope`], [`submitted_request_scope`], [`charge_scope`],
/// [`billing_adjustment_scope`], [`invoice_line_item_scope`] and the
/// agreement-keyed scopes of `GetAgreementTerms` and `GetAgreementEntitlements` —
/// is built from it.
pub fn listing_scope(operation: &str, selectors: &[&str]) -> String {
    let mut parts = Vec::with_capacity(selectors.len() + 1);
    parts.push(operation);
    parts.extend_from_slice(selectors);
    query_scope(&parts)
}

/// The scope a `SearchAgreements` page token belongs to: the whole query, so a
/// token cannot resume a different search.
pub fn search_scope(
    operation: &str,
    catalog: Option<&str>,
    filters: &[Filter],
    sort_by: &str,
    sort_order: &str,
) -> String {
    let mut pairs: Vec<String> = filters
        .iter()
        .map(|filter| {
            query_scope(&[
                filter.name.as_deref().unwrap_or_default(),
                filter
                    .values
                    .as_ref()
                    .and_then(|values| values.first())
                    .map(String::as_str)
                    .unwrap_or_default(),
            ])
        })
        .collect();
    pairs.sort();
    listing_scope(
        operation,
        &[
            catalog.unwrap_or_default(),
            sort_by,
            sort_order,
            &pairs.join(""),
        ],
    )
}

/// The scope a page token of a listing over seller-submitted requests belongs
/// to — `ListAgreementPaymentRequests` and `ListAgreementCancellationRequests`,
/// which take the same five selectors over the same two kinds of request
/// ([`submitted_request_party_matches`] decides the party for both).
///
/// Every parameter that selects which requests the list reports is a part, so a
/// token minted for one query cannot resume another, and `operation` is a part
/// too, so the two listings never hash alike however their selectors line up.
///
/// Built from [`listing_scope`] like every other scope in this module, so no
/// value a caller supplies can imitate the delimiters and make two different
/// listings hash alike.
pub fn submitted_request_scope(
    operation: &str,
    party_type: &str,
    agreement_id: Option<&str>,
    agreement_type: Option<&str>,
    catalog: Option<&str>,
    status: Option<&str>,
) -> String {
    listing_scope(
        operation,
        &[
            party_type,
            agreement_id.unwrap_or_default(),
            agreement_type.unwrap_or_default(),
            catalog.unwrap_or_default(),
            status.unwrap_or_default(),
        ],
    )
}

/// The scope a `ListAgreementCharges` page token belongs to: every parameter
/// that selects which charges the list reports, so a token minted for one list
/// cannot resume another.
///
/// Built from [`listing_scope`] like every other scope in this module, so no
/// value a caller supplies can imitate the delimiters and make two different
/// listings hash alike.
pub fn charge_scope(
    operation: &str,
    agreement_id: Option<&str>,
    agreement_type: Option<&str>,
    catalog: Option<&str>,
) -> String {
    listing_scope(
        operation,
        &[
            agreement_id.unwrap_or_default(),
            agreement_type.unwrap_or_default(),
            catalog.unwrap_or_default(),
        ],
    )
}
