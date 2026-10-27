// SPDX-License-Identifier: CC0-1.0

//! @op DescribeSecurityGroupVpcAssociations
//! Reports the security group VPC associations `AssociateSecurityGroupVpc` made, out of the
//! store that operation writes — so the two cannot report one association differently.
//!
//! Implemented together with `AssociateSecurityGroupVpc`, whose stub issue this change
//! closes: a write with no reader cannot be read back.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeSecurityGroupVpcAssociations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes security group VPC associations made with AssociateSecurityGroupVpc.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — Security group VPC association filters. group-id: The security
//!   group ID. group-owner-id: The group owner ID. state: The state of the association.
//!   vpc-id: The ID of the associated VPC. vpc-owner-id: The account ID of the VPC owner.
//! * `MaxResults` (DescribeSecurityGroupVpcAssociationsMaxResults, range 5..1000) — The
//!   maximum number of items to return for this request. To get the next page of items, make
//!   another request with the token returned in the output. For more information, see
//!   Pagination.
//! * `NextToken` (String) — The token returned from a previous paginated request. Pagination
//!   continues from the end of the items returned by the previous request.
//!
//! ## Output
//!
//! * `NextToken` (String) — The token to include in another request to get the next page of
//!   items. This value is null when there are no more items to return.
//! * `SecurityGroupVpcAssociations` (SecurityGroupVpcAssociationList) — The security group
//!   VPC associations.

#[allow(unused_imports)]
use super::types::*;

use std::borrow::Cow;

use crate::state::{
    CompiledFilter, MAX_FILTERS, MAX_FILTER_MATCH_STEPS, MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES,
    MAX_PAGE_SIZE, MatchBudget,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeSecurityGroupVpcAssociationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeSecurityGroupVpcAssociationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeSecurityGroupVpcAssociations")]
#[derive(Debug)]
pub enum DescribeSecurityGroupVpcAssociationsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_security_group_vpc_associations::DescribeSecurityGroupVpcAssociationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeSecurityGroupVpcAssociationsError = crate::scaffolded::Scaffolded<DescribeSecurityGroupVpcAssociationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeSecurityGroupVpcAssociationsErrorModeled> for DescribeSecurityGroupVpcAssociationsError {
    fn from(modeled: DescribeSecurityGroupVpcAssociationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeSecurityGroupVpcAssociations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeSecurityGroupVpcAssociations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeSecurityGroupVpcAssociations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeSecurityGroupVpcAssociations<Eng>
{
    type Input = DescribeSecurityGroupVpcAssociationsInput;
    type Output = DescribeSecurityGroupVpcAssociationsOutput;
    type Error = DescribeSecurityGroupVpcAssociationsError;

    const NAME: &'static str = "DescribeSecurityGroupVpcAssociations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_SECURITY_GROUP_VPC_ASSOCIATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_SECURITY_GROUP_VPC_ASSOCIATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata only; this operation reads none of it. The store
        // is reached through the engine held on `self`.
        let _ = ctx;

        // "Checks whether you have the required permissions for the action, without
        // actually making the request, and provides an error response. If you have the
        // required permissions, the error response is DryRunOperation" (`$DryRun`). Every
        // caller of this emulator is permitted, so a dry run reports that outcome and reads
        // nothing.
        if input.dry_run.unwrap_or(false) {
            return Err(crate::state::dry_run_operation());
        }

        let state = crate::state::require_service_state(&*self.engine)?;

        // "The maximum number of items to return for this request"
        // (`DescribeSecurityGroupVpcAssociationsMaxResults`, `range 5..1000`). The model's
        // own range is refused by the boundary validator before this handler runs; asking it
        // here too is what keeps the page this handler serves inside the published range if
        // that ever changes, and a value below the minimum is REFUSED rather than silently
        // rewritten — the way every other paginated describe of this crate that enforces the
        // bound itself refuses it, in the same wording. The upper end is not an error: a
        // page size above the maximum serves the maximum.
        if input
            .max_results
            .is_some_and(|max_results| max_results < MIN_PAGE_SIZE)
        {
            return Err(crate::state::invalid_parameter_value(format!(
                "Invalid value '{}' for MaxResults. It must be between {MIN_PAGE_SIZE} and \
                 {MAX_PAGE_SIZE}",
                input.max_results.unwrap_or_default()
            )));
        }

        // "The token returned from a previous paginated request" (`$NextToken`) — anything
        // else is refused rather than read as a cursor, and it is refused before the store is
        // consulted and before the `Filters` are read, so whether a bogus token is caught
        // never depends on what the request asks to select. The one member read before it is
        // `DryRun`, which asks about the request as a whole rather than selecting anything.
        let start_at = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_page_token(token)
                    .ok_or_else(|| crate::state::invalid_pagination_token_for(token))?,
            ),
        };

        // Matching cost is filters x values x value length x stored associations, and the
        // model bounds none of them (`FilterList` and `Filter$Values` carry no `@length`),
        // so every request-controlled factor is capped HERE, before any work proportional
        // to it is done and before the store is locked — the same ceilings every other
        // filtered describe of this crate enforces, from `crate::state`.
        let filters: &[Filter] = input.filters.as_deref().unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(crate::state::invalid_parameter_value(format!(
                "The request carries {} filters, above the maximum of {MAX_FILTERS}",
                filters.len()
            )));
        }
        // A filter name this operation does not document is refused whether or not any
        // association would have matched it, so an unsupported filter can never read as "no
        // results". Each value is compiled into its pattern ONCE here, rather than
        // recompiled for every candidate association.
        let mut compiled: Vec<CompiledFilter> = Vec::with_capacity(filters.len());
        for filter in filters {
            let name = filter.name.as_deref().unwrap_or_default();
            let canonical = canonical_filter_name(name).ok_or_else(|| {
                crate::state::invalid_parameter_value(format!("The filter '{name}' is invalid"))
            })?;
            let values: &[String] = filter.values.as_deref().unwrap_or_default();
            if values.len() > MAX_FILTER_VALUES {
                return Err(crate::state::invalid_parameter_value(format!(
                    "The filter '{name}' carries {} values, above the maximum of \
                     {MAX_FILTER_VALUES}",
                    values.len()
                )));
            }
            // A capped NUMBER of comparisons is not capped WORK: the matcher's cost is the
            // product of the pattern's length and the value's, so a value's size is bounded
            // here too.
            if let Some(oversize) = values
                .iter()
                .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
            {
                return Err(crate::state::invalid_parameter_value(format!(
                    "A value of the filter '{name}' carries {} bytes, above the maximum of \
                     {MAX_FILTER_VALUE_BYTES}",
                    oversize.len()
                )));
            }
            compiled.push(CompiledFilter::new(canonical.to_string(), values.to_vec()));
        }

        let store = state.lock();

        // "Describes security group VPC associations made with AssociateSecurityGroupVpc"
        // (`DescribeSecurityGroupVpcAssociations`): every association the store holds, each
        // in the state the CLOCK has reached — reading one never moves it, and never
        // creates one. Every filter must then hold ("If you specify multiple filters, the
        // filters are joined with an AND" — `Filter`), and every comparison is charged to
        // ONE budget, so the product of stored associations, filters, values and value
        // length is bounded however the caller combines them.
        let now = ::std::time::Instant::now();
        let mut budget = MatchBudget::new(MAX_FILTER_MATCH_STEPS);
        let mut selected: Vec<SecurityGroupVpcAssociation> = Vec::new();
        for association in store.security_group_vpc_associations_at(now) {
            if budget.exhausted() {
                break;
            }
            if crate::state::matches_every_filter(&compiled, &mut budget, &|name| {
                filter_values(&association, name)
            }) {
                selected.push(association);
            }
        }
        // A spent budget means the answer is INCOMPLETE, which is not something to report
        // as a result: the request asked for more filtering than one call will do, and is
        // told so. Paging is NOT the remedy — the filters are evaluated in full before
        // `MaxResults` cuts the page — so the refusal names the one thing that does reduce
        // the comparisons: fewer filters and fewer values.
        if budget.exhausted() {
            return Err(crate::scaffolded::Scaffolded::request_limit_exceeded(
                format!(
                    "The request's filters would take more than {MAX_FILTER_MATCH_STEPS} \
                     comparisons to evaluate; narrow them by naming fewer filters or fewer \
                     values"
                ),
            ));
        }

        // Page in (`GroupId`, `VpcId`) order, which [`page_cursor`] names a position in.
        // The store is an ordered map keyed by that pair, so its iteration order already
        // satisfies this sort; doing it here establishes the invariant the cursor depends on
        // at the point of use rather than inheriting it from the container `crate::state`
        // happens to hold.
        selected.sort_by_cached_key(page_cursor);

        // That order is both the stable paging order and the cursor, so a token naming the
        // pair the next page starts at always advances.
        let from = match start_at.as_deref() {
            None => 0,
            Some(cursor) => {
                selected.partition_point(|association| page_cursor(association).as_str() < cursor)
            }
        };
        let page = selected.get(from..).unwrap_or_default();

        // "The maximum number of items to return for this request. To get the next page of
        // items, make another request with the token returned in the output" (`$MaxResults`).
        // A request that names NO page size gets the documented maximum rather than the whole
        // store — the same default every other paginated describe of this crate applies
        // ([`MAX_PAGE_SIZE`]); the rest follow through `NextToken`, so nothing is withheld. A
        // value below the minimum was refused above, and one above the maximum serves the
        // maximum, which is also what a value too large to convert does.
        let limit = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE);

        // "This value is null when there are no more items to return" (`$NextToken`): a
        // token only when an association follows this page.
        let next_token = page
            .get(limit)
            .map(|association| crate::state::encode_page_token(&page_cursor(association)));

        Ok(DescribeSecurityGroupVpcAssociationsOutput {
            security_group_vpc_associations: Some(page.iter().take(limit).cloned().collect()),
            next_token,
        })
    }
}

/// The smallest page `DescribeSecurityGroupVpcAssociations` serves: the minimum of the range
/// its own `MaxResults` publishes (`DescribeSecurityGroupVpcAssociationsMaxResults`,
/// `range 5..1000`, whose maximum is the crate-wide [`MAX_PAGE_SIZE`]). Typed as the member
/// is, so the request's value is compared against it without a conversion.
const MIN_PAGE_SIZE: i32 = 5;

/// The filter names this operation documents, from
/// `DescribeSecurityGroupVpcAssociationsRequest$Filters`:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeSecurityGroupVpcAssociationsRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// That doc publishes no `tag:` filter for this operation — an association carries no tags —
/// so the catalog is closed and every entry must be answered by [`filter_values`].
const SECURITY_GROUP_VPC_ASSOCIATION_FILTERS: [&str; 5] = [
    "group-id",
    "group-owner-id",
    "state",
    "vpc-id",
    "vpc-owner-id",
];

/// The name [`filter_values`] answers for the filter the request called `name`, or `None`
/// when this operation documents no such filter.
///
/// "Filter names are case-sensitive" (`Filter$Name`), so every comparison is exact. This
/// operation's `Filters` doc publishes no alternative spellings.
fn canonical_filter_name(name: &str) -> Option<&str> {
    SECURITY_GROUP_VPC_ASSOCIATION_FILTERS
        .contains(&name)
        .then_some(name)
}

/// The values the EC2 filter `name` compares against this association.
///
/// A filter naming an attribute this association does not carry compares against NO values,
/// so it selects nothing. `name` is a name [`canonical_filter_name`] resolved.
fn filter_values<'a>(
    association: &'a SecurityGroupVpcAssociation,
    name: &str,
) -> Vec<Cow<'a, str>> {
    let text = |value: Option<&'a str>| -> Vec<Cow<'a, str>> {
        value.map(Cow::Borrowed).into_iter().collect()
    };
    match name {
        "group-id" => text(association.group_id.as_deref()),
        "group-owner-id" => text(association.group_owner_id.as_deref()),
        "state" => text(association.state.as_deref()),
        "vpc-id" => text(association.vpc_id.as_deref()),
        "vpc-owner-id" => text(association.vpc_owner_id.as_deref()),
        // Unreachable: `canonical_filter_name` has already refused every other name. An
        // undefined name compares against nothing, so it could never widen a result even
        // if that check were bypassed.
        _ => Vec::new(),
    }
}

/// The position of one association in the order this operation pages in: its security group
/// id, then the id of the VPC it is associated with — the pair the association IS, and the
/// key its store is ordered by.
///
/// The separator sorts ahead of every character an EC2 resource id can carry, so one group
/// id being a prefix of another cannot reorder two associations — which is what lets a
/// cursor be compared against this string directly.
fn page_cursor(association: &SecurityGroupVpcAssociation) -> String {
    format!(
        "{} {}",
        association.group_id.as_deref().unwrap_or_default(),
        association.vpc_id.as_deref().unwrap_or_default()
    )
}
