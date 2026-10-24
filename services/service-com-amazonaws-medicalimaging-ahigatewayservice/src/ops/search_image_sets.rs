// SPDX-License-Identifier: CC0-1.0

//! @op SearchImageSets
//!
//! Reports, from persisted state, the image sets of ONE data store whose stored
//! attributes satisfy EVERY filter the search criteria carry, in the order the criteria
//! ask for, one `maxResults`-sized page at a time with an opaque token that resumes the
//! walk where the previous page stopped.
//!
//! A REPORTING operation, so it writes nothing of its own: a search reports each image
//! set's `lastAccessedAt` from state without moving it, because searching an image set
//! is not an access of the DICOM content it holds. Like every handler it calls
//! [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`]
//! first, which materialises only a transition the CLOCK already owed at that instant.
//!
//! # What a filter means
//!
//! A filter names one searchable attribute and the operator to compare it with, and the
//! filters are ANDed: an image set is reported only when every one of them holds of it.
//!
//! * `EQUAL` takes exactly ONE attribute value — the input documents the criteria as
//!   "filters by applying a maximum of 1 item to SearchByAttribute" — and holds when the
//!   image set's stored attribute equals it. An attribute the image set does not carry
//!   equals nothing, so it matches no `EQUAL` filter.
//! * `BETWEEN` takes a lower bound and an upper bound of the SAME date-or-time attribute
//!   — "All date range queries must be input as (lowerBound, upperBound)" — and holds
//!   when the stored attribute lies in the inclusive range they bound. The operator is
//!   documented as the one "for `imageSetDateTime`", so a range over an attribute that
//!   is not a date or a time, bounds naming two different attributes, and a lower bound
//!   past its upper bound are each refused with the modelled `ValidationException`
//!   (HTTP 400) rather than answered with an empty page.
//!
//! A study date and time bound that names no `DICOMStudyTime` bounds the whole study
//! DATE, which is what makes a one-day range expressible with the date alone.
//!
//! # The order, and where a page resumes
//!
//! "By default, SearchImageSets uses the updatedAt field for sorting in descending order
//! from newest to oldest" — so a request naming no `sort` is served that order, and the
//! response always echoes the sort it was served in. The image set identifier breaks a
//! tie, so the order is total and a page boundary is stable however the underlying map
//! iterates. The token carries the FULL sort key the next page starts at (see
//! [`crate::state::encode_keyed_page_token`]) and is marked with the data store AND the
//! order it was minted under, so a token honoured under another sort — which would
//! silently skip or repeat image sets — is refused instead.
//!
//! Only the image sets the data store still HOLDS are searchable: a retired one keeps
//! nothing but the properties `GetImageSet` reports of a tombstone, so there is no
//! content left for a search to match or report.
//!
//! ---
//!
//! # Modeled contract for `SearchImageSets`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Search image sets based on defined input attributes. SearchImageSets accepts a single
//! search query parameter and returns a paginated response of all image sets that have the
//! matching criteria. All date range queries must be input as (lowerBound, upperBound). By
//! default, SearchImageSets uses the updatedAt field for sorting in descending order from
//! newest to oldest.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The identifier of the
//!   data store where the image sets reside.
//! * `maxResults` (Integer) — The maximum number of results that can be returned in a search.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The token used for
//!   pagination of results returned in the response. Use the token returned from the previous
//!   request to continue results where the previous request ended.
//! * `searchCriteria` (SearchCriteria) — The search criteria that filters by applying a
//!   maximum of 1 item to SearchByAttribute.
//!
//! ## Output
//!
//! * `imageSetsMetadataSummaries` (ImageSetsMetadataSummaries) **required** — The model
//!   containing the image set results.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The token for
//!   pagination results.
//! * `sort` (Sort) — The sort order for image set search results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Updating or deleting a resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

use std::cmp::Ordering;
use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;

use crate::state::ImageSet;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SearchImageSetsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SearchImageSetsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#SearchImageSets")]
#[derive(Debug)]
pub enum SearchImageSetsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::search_image_sets::SearchImageSetsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SearchImageSetsError = crate::scaffolded::Scaffolded<SearchImageSetsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SearchImageSetsErrorModeled> for SearchImageSetsError {
    fn from(modeled: SearchImageSetsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SearchImageSets` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SearchImageSets<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SearchImageSets<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SearchImageSets<Eng>
{
    type Input = SearchImageSetsInput;
    type Output = SearchImageSetsOutput;
    type Error = SearchImageSetsError;

    const NAME: &'static str = "SearchImageSets";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SEARCH_IMAGE_SETS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SEARCH_IMAGE_SETS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        // The criteria are read BEFORE any state: a filter this service cannot evaluate
        // is a client error whatever the data store holds, and an empty page would dress
        // a request it never understood as an answer.
        let criteria = input.search_criteria.as_ref();
        let predicates = criteria
            .and_then(|criteria| criteria.filters.as_deref())
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, filter)| Predicate::from_filter(index, filter))
            .collect::<Result<Vec<Predicate>, String>>()
            .map_err(invalid_request)?;
        let sort = EffectiveSort::from_criteria(criteria.and_then(|criteria| criteria.sort.as_ref()))
            .map_err(invalid_request)?;

        // The token is this service's own opaque handle, marked with the data store and
        // the ORDER the position belongs to; only one this service minted for that pair
        // can be honoured, and anything else is a client error rather than an empty page.
        // The refusal is the service's FIXED sentence: it does not vary with the token,
        // so it reveals nothing about how a real one is built.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                sort.decode_page_token(&input.datastore_id, token)
                    .ok_or_else(|| {
                        invalid_request(crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                    })?,
            ),
        };

        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            // Clamped here, not merely at the boundary: one request must never be able
            // to materialise an unbounded page.
            .map(|requested| {
                requested.clamp(
                    crate::state::MIN_LIST_PAGE_SIZE,
                    crate::state::MAX_LIST_PAGE_SIZE,
                )
            })
            .unwrap_or(crate::state::MAX_LIST_PAGE_SIZE);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            SearchImageSetsErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all
        // (`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::datastore_in`).
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            SearchImageSetsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        // Every image set the data store still holds that satisfies every filter, paired
        // with the key the requested order ranks it by.
        let mut matched: Vec<(SortKey, &ImageSet)> = datastore
            .image_sets
            .values()
            .filter(|image_set| !image_set.is_deleted())
            .filter(|image_set| {
                predicates
                    .iter()
                    .all(|predicate| predicate.matches(image_set))
            })
            .map(|image_set| (sort.key_of(image_set), image_set))
            .collect();
        // Total order over the whole result: the requested field, the image set
        // identifier to break a tie — sorted here rather than trusting the order the
        // records are held in, so a page boundary is stable.
        matched.sort_by(|left, right| {
            (&left.0, left.1.image_set_id.as_str()).cmp(&(&right.0, right.1.image_set_id.as_str()))
        });
        if sort.order == SortOrder::Descending {
            matched.reverse();
        }

        // Resume at the first image set the walk reaches at or past the token's position.
        // Comparing the position — not an index into a page already served — is what
        // keeps the walk correct when the collection changed between pages: an image set
        // minted after the token can neither displace one never seen nor repeat one
        // served, and one deleted since does not strand the resume point.
        let start = match &resume_from {
            None => 0,
            Some((cursor_key, cursor_id)) => matched
                .iter()
                .position(|(key, image_set)| {
                    sort.reaches(
                        (key, image_set.image_set_id.as_str()),
                        (cursor_key, cursor_id.as_str()),
                    )
                })
                .unwrap_or(matched.len()),
        };

        let end = start.saturating_add(page_size).min(matched.len());
        let image_sets_metadata_summaries: Vec<ImageSetsMetadataSummary> = matched[start..end]
            .iter()
            .map(|(_, image_set)| summary_of(image_set))
            .collect();

        // A token only when a further page exists, so a caller's walk terminates on the
        // page that exhausts the result.
        let next_token = matched.get(end).map(|(key, image_set)| {
            sort.encode_page_token(&input.datastore_id, key, &image_set.image_set_id)
        });

        Ok(SearchImageSetsOutput {
            image_sets_metadata_summaries,
            // The order the result was served in, which is the default one when the
            // request named none.
            sort: Some(sort.reported()),
            next_token,
        })
    }
}

/// The modelled `ValidationException` (HTTP 400) for a request this handler cannot
/// evaluate, worded through this service's one envelope.
fn invalid_request(message: String) -> SearchImageSetsError {
    SearchImageSetsErrorModeled::ValidationException(ValidationException { message }).into()
}

/// What one image set reports in a search result.
///
/// Every member comes from the record: the version is the image set's LATEST, the
/// timestamps are the image set's own creation, that version's last change and the last
/// access of the content the image set holds, and the DICOM attributes are the ones the
/// image set holds.
fn summary_of(image_set: &ImageSet) -> ImageSetsMetadataSummary {
    ImageSetsMetadataSummary {
        image_set_id: image_set.image_set_id.clone(),
        version: i32::try_from(image_set.latest_version.version_id).ok(),
        created_at: Some(crate::state::to_date_time(image_set.created_at)),
        updated_at: Some(crate::state::to_date_time(image_set.latest_version.updated_at)),
        // The same instant `GetImageSet` reports for this image set, read from state:
        // a search does not touch the content it describes, so it records no access.
        last_accessed_at: Some(crate::state::to_date_time(image_set.last_accessed_at)),
        storage_tier: Some(crate::state::DEFAULT_STORAGE_TIER.to_string()),
        dicom_tags: Some(image_set.dicom_tags.clone()),
        is_primary: Some(image_set.is_primary),
    }
}

/// `SortField`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="SortField")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortField {
    UpdatedAt,
    CreatedAt,
    StudyDateAndTime,
}

impl SortField {
    /// The wire form of this field.
    fn as_wire(self) -> &'static str {
        match self {
            Self::UpdatedAt => "updatedAt",
            Self::CreatedAt => "createdAt",
            Self::StudyDateAndTime => "DICOMStudyDateAndTime",
        }
    }

    /// Parse a wire field, or `None` when the string names no modelled value.
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "updatedAt" => Some(Self::UpdatedAt),
            "createdAt" => Some(Self::CreatedAt),
            "DICOMStudyDateAndTime" => Some(Self::StudyDateAndTime),
            _ => None,
        }
    }
}

/// `SortOrder`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="SortOrder")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    /// The wire form of this order.
    fn as_wire(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }

    /// Parse a wire order, or `None` when the string names no modelled value.
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ASC" => Some(Self::Ascending),
            "DESC" => Some(Self::Descending),
            _ => None,
        }
    }
}

/// The order a search walks, which is the one the response reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffectiveSort {
    field: SortField,
    order: SortOrder,
}

impl EffectiveSort {
    /// The sort the request asks for, or the documented default — "By default,
    /// SearchImageSets uses the updatedAt field for sorting in descending order from
    /// newest to oldest" — when it names none.
    ///
    /// A value outside either enum is refused rather than silently defaulted: the
    /// boundary validator already states the enum, so reaching here with another value
    /// means a caller got past it, and answering a sort nobody asked for would order the
    /// result by something the caller never named.
    fn from_criteria(sort: Option<&Sort>) -> Result<Self, String> {
        let Some(sort) = sort else {
            return Ok(Self {
                field: SortField::UpdatedAt,
                order: SortOrder::Descending,
            });
        };
        let field = SortField::from_wire(&sort.sort_field).ok_or_else(|| {
            crate::state::handler_requirement_error(
                "searchCriteria.sort.sortField",
                Some(&sort.sort_field),
                "Member must satisfy enum value set [updatedAt, createdAt, DICOMStudyDateAndTime]",
            )
        })?;
        let order = SortOrder::from_wire(&sort.sort_order).ok_or_else(|| {
            crate::state::handler_requirement_error(
                "searchCriteria.sort.sortOrder",
                Some(&sort.sort_order),
                "Member must satisfy enum value set [ASC, DESC]",
            )
        })?;
        Ok(Self { field, order })
    }

    /// This sort as the response reports it.
    fn reported(self) -> Sort {
        Sort {
            sort_field: self.field.as_wire().to_string(),
            sort_order: self.order.as_wire().to_string(),
        }
    }

    /// The key this sort ranks `image_set` by.
    fn key_of(self, image_set: &ImageSet) -> SortKey {
        match self.field {
            SortField::UpdatedAt => instant_key(image_set.latest_version.updated_at),
            SortField::CreatedAt => instant_key(image_set.created_at),
            SortField::StudyDateAndTime => SortKey::Text(study_rank(
                image_set.dicom_tags.dicom_study_date.as_deref(),
                image_set.dicom_tags.dicom_study_time.as_deref(),
            )),
        }
    }

    /// Whether `candidate` is at or past `cursor` IN THE ORDER THIS SORT WALKS — which is
    /// what makes the same resume rule serve both directions.
    fn reaches(self, candidate: (&SortKey, &str), cursor: (&SortKey, &str)) -> bool {
        match self.order {
            SortOrder::Ascending => candidate >= cursor,
            SortOrder::Descending => candidate <= cursor,
        }
    }

    /// The marker every token this search mints carries: this operation's own prefix, the
    /// data store the position belongs to, and the ORDER it is a position in.
    ///
    /// A hex string this service never issued, one minted for another collection, one
    /// minted against another data store and one minted under another sort are all
    /// refused rather than honoured as a resume point — a position in one order is not a
    /// position in another.
    fn page_token_marker(self, datastore_id: &str) -> String {
        format!(
            "ahi-image-set-search-v1:{datastore_id}:{}:{}:",
            self.field.as_wire(),
            self.order.as_wire()
        )
    }

    /// Encode the sort key the next page starts at as this search's opaque `nextToken`.
    fn encode_page_token(self, datastore_id: &str, key: &SortKey, image_set_id: &str) -> String {
        crate::state::encode_keyed_page_token(
            &self.page_token_marker(datastore_id),
            &key.as_text(),
            image_set_id,
        )
    }

    /// Decode a token [`EffectiveSort::encode_page_token`] produced for this data store
    /// and this order, or `None` when it was not one.
    fn decode_page_token(self, datastore_id: &str, token: &str) -> Option<(SortKey, String)> {
        let (key, image_set_id) = crate::state::decode_keyed_page_token(
            &self.page_token_marker(datastore_id),
            token,
        )?;
        Some((SortKey::parse(self.field, &key)?, image_set_id))
    }
}

/// Where one image set sits in the requested order.
///
/// Two variants because the sort fields rank by two different things — an instant, or a
/// DICOM study date and time — and within one request every key is the same variant, so
/// the two are never compared against each other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    /// Seconds and nanoseconds since the epoch, compared as numbers.
    Instant(i64, u32),
    /// A study date and time, compared as text — which for the fixed-width numeric forms
    /// the model documents (`yyMMdd` and `HHmmss.FFFFFF`) is chronological order.
    Text(String),
}

impl SortKey {
    /// This key as the text a `nextToken` carries.
    fn as_text(&self) -> String {
        match self {
            Self::Instant(secs, nanos) => format!("{secs}.{nanos}"),
            Self::Text(text) => text.clone(),
        }
    }

    /// The key [`SortKey::as_text`] wrote, read back for the field it was minted under,
    /// or `None` when the text is not one this service would have written for that field.
    fn parse(field: SortField, text: &str) -> Option<Self> {
        match field {
            SortField::UpdatedAt | SortField::CreatedAt => {
                let (secs, nanos) = text.split_once('.')?;
                Some(Self::Instant(secs.parse().ok()?, nanos.parse().ok()?))
            }
            SortField::StudyDateAndTime => Some(Self::Text(text.to_string())),
        }
    }
}

/// The separator between a study date and its time in a [`SortKey::Text`].
///
/// A control character, which sorts below every character either member can hold, so
/// comparing the joined text compares the date first and only then the time — the
/// ordering of the pair, not of two strings glued together.
const STUDY_RANK_SEPARATOR: char = '\u{1f}';

/// The rank of a stored study date and time. An image set carrying neither ranks below
/// every image set carrying either, because an absent value is the lowest.
fn study_rank(study_date: Option<&str>, study_time: Option<&str>) -> String {
    format!(
        "{}{STUDY_RANK_SEPARATOR}{}",
        study_date.unwrap_or_default(),
        study_time.unwrap_or_default()
    )
}

/// The key an instant ranks by, to full nanosecond precision.
fn instant_key(instant: SystemTime) -> SortKey {
    let (secs, nanos) = instant_pair(instant);
    SortKey::Instant(secs, nanos)
}

/// A modelled timestamp as the pair an instant is compared by.
fn date_time_key(at: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// One filter, as the comparison it stands for.
#[derive(Debug, Clone)]
enum Predicate {
    /// The stored attribute must EQUAL this value.
    Equal(SearchByAttributeValue),
    /// The stored attribute must lie in this inclusive range.
    Between(RangeFilter),
}

impl Predicate {
    /// Read the filter at `index` of the criteria, or state why it cannot be evaluated.
    fn from_filter(index: usize, filter: &SearchFilter) -> Result<Self, String> {
        let values_path = format!("searchCriteria.filters[{index}].values");
        match filter.operator.as_str() {
            "EQUAL" => {
                // "filters by applying a maximum of 1 item to SearchByAttribute": an
                // equality holds of one value, so two of them state no comparison.
                let [value] = filter.values.as_slice() else {
                    return Err(crate::state::handler_requirement_error(
                        &values_path,
                        None,
                        "Member must hold exactly 1 search attribute value when the operator is EQUAL",
                    ));
                };
                Ok(Self::Equal(value.clone()))
            }
            "BETWEEN" => {
                // "All date range queries must be input as (lowerBound, upperBound)".
                let [lower, upper] = filter.values.as_slice() else {
                    return Err(crate::state::handler_requirement_error(
                        &values_path,
                        None,
                        "Member must hold a lower bound and an upper bound when the operator is BETWEEN",
                    ));
                };
                let range = RangeFilter::new(lower, upper).ok_or_else(|| {
                    crate::state::handler_requirement_error(
                        &values_path,
                        None,
                        "Member must bound one date or time attribute when the operator is BETWEEN",
                    )
                })?;
                if !range.is_ordered() {
                    return Err(crate::state::handler_requirement_error(
                        &values_path,
                        None,
                        "Member must hold the lower bound before the upper bound",
                    ));
                }
                Ok(Self::Between(range))
            }
            other => Err(crate::state::handler_requirement_error(
                &format!("searchCriteria.filters[{index}].operator"),
                Some(other),
                "Member must satisfy enum value set [EQUAL, BETWEEN]",
            )),
        }
    }

    /// Whether this filter holds of `image_set`.
    fn matches(&self, image_set: &ImageSet) -> bool {
        let tags = &image_set.dicom_tags;
        match self {
            // An attribute the image set does not carry equals nothing.
            Self::Equal(value) => match value {
                SearchByAttributeValue::DICOMPatientId(wanted) => {
                    tags.dicom_patient_id.as_deref() == Some(wanted.as_str())
                }
                SearchByAttributeValue::DICOMAccessionNumber(wanted) => {
                    tags.dicom_accession_number.as_deref() == Some(wanted.as_str())
                }
                SearchByAttributeValue::DICOMStudyId(wanted) => {
                    tags.dicom_study_id.as_deref() == Some(wanted.as_str())
                }
                SearchByAttributeValue::DICOMStudyInstanceUID(wanted) => {
                    tags.dicom_study_instance_uid.as_deref() == Some(wanted.as_str())
                }
                SearchByAttributeValue::DICOMSeriesInstanceUID(wanted) => {
                    tags.dicom_series_instance_uid.as_deref() == Some(wanted.as_str())
                }
                SearchByAttributeValue::createdAt(wanted) => {
                    date_time_key(wanted) == instant_pair(image_set.created_at)
                }
                SearchByAttributeValue::updatedAt(wanted) => {
                    date_time_key(wanted) == instant_pair(image_set.latest_version.updated_at)
                }
                SearchByAttributeValue::DICOMStudyDateAndTime(wanted) => {
                    compare_study(tags, wanted) == Some(Ordering::Equal)
                }
                SearchByAttributeValue::isPrimary(wanted) => image_set.is_primary == *wanted,
            },
            Self::Between(range) => range.contains(image_set),
        }
    }
}

/// An inclusive range over ONE ordered attribute — the only attributes a range can bound,
/// the operator being documented as the one "for `imageSetDateTime`".
#[derive(Debug, Clone)]
enum RangeFilter {
    CreatedAt {
        lower: (i64, u32),
        upper: (i64, u32),
    },
    UpdatedAt {
        lower: (i64, u32),
        upper: (i64, u32),
    },
    StudyDateAndTime {
        lower: DICOMStudyDateAndTime,
        upper: DICOMStudyDateAndTime,
    },
}

impl RangeFilter {
    /// The range two bounds state, or `None` when they do not state one: bounds naming
    /// two different attributes, or an attribute that is not a date or a time and so has
    /// no range to lie in.
    fn new(lower: &SearchByAttributeValue, upper: &SearchByAttributeValue) -> Option<Self> {
        match (lower, upper) {
            (
                SearchByAttributeValue::createdAt(lower),
                SearchByAttributeValue::createdAt(upper),
            ) => Some(Self::CreatedAt {
                lower: date_time_key(lower),
                upper: date_time_key(upper),
            }),
            (
                SearchByAttributeValue::updatedAt(lower),
                SearchByAttributeValue::updatedAt(upper),
            ) => Some(Self::UpdatedAt {
                lower: date_time_key(lower),
                upper: date_time_key(upper),
            }),
            (
                SearchByAttributeValue::DICOMStudyDateAndTime(lower),
                SearchByAttributeValue::DICOMStudyDateAndTime(upper),
            ) => Some(Self::StudyDateAndTime {
                lower: lower.clone(),
                upper: upper.clone(),
            }),
            _ => None,
        }
    }

    /// Whether the bounds are in the order the documentation requires, `(lowerBound,
    /// upperBound)`.
    fn is_ordered(&self) -> bool {
        match self {
            Self::CreatedAt { lower, upper } | Self::UpdatedAt { lower, upper } => lower <= upper,
            Self::StudyDateAndTime { lower, upper } => {
                study_rank(Some(&lower.dicom_study_date), lower.dicom_study_time.as_deref())
                    <= study_rank(Some(&upper.dicom_study_date), upper.dicom_study_time.as_deref())
            }
        }
    }

    /// Whether `image_set`'s stored attribute lies in this range. An image set carrying no
    /// study date lies in no study-date range: there is nothing to place against the
    /// bounds.
    fn contains(&self, image_set: &ImageSet) -> bool {
        match self {
            Self::CreatedAt { lower, upper } => {
                (*lower..=*upper).contains(&instant_pair(image_set.created_at))
            }
            Self::UpdatedAt { lower, upper } => (*lower..=*upper)
                .contains(&instant_pair(image_set.latest_version.updated_at)),
            Self::StudyDateAndTime { lower, upper } => {
                let tags = &image_set.dicom_tags;
                matches!(
                    compare_study(tags, lower),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    compare_study(tags, upper),
                    Some(Ordering::Less | Ordering::Equal)
                )
            }
        }
    }
}

/// The instant an image set's timestamp is compared by.
fn instant_pair(instant: SystemTime) -> (i64, u32) {
    let at = crate::state::to_date_time(instant);
    (at.secs(), at.subsec_nanos())
}

/// Where the study date and time an image set holds sits against `bound`, or `None` when
/// the image set holds no study date and so cannot be placed against one.
///
/// A bound that names no `DICOMStudyTime` bounds the whole study DATE: only the dates are
/// compared, so every study of that date is inside it whatever its time.
fn compare_study(tags: &DICOMTags, bound: &DICOMStudyDateAndTime) -> Option<Ordering> {
    let stored_date = tags.dicom_study_date.as_deref()?;
    let by_date = stored_date.cmp(bound.dicom_study_date.as_str());
    if by_date != Ordering::Equal {
        return Some(by_date);
    }
    let Some(bound_time) = bound.dicom_study_time.as_deref() else {
        return Some(Ordering::Equal);
    };
    Some(
        tags.dicom_study_time
            .as_deref()
            .unwrap_or_default()
            .cmp(bound_time),
    )
}
