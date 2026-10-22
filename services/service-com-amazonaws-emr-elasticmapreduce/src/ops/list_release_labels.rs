// SPDX-License-Identifier: CC0-1.0

//! @op ListReleaseLabels
//!
//! Reports the Amazon EMR release catalog of the Region the request is addressed
//! to: the release labels this service publishes, newest release first, narrowed
//! by the `Prefix` and `Application` filters and cut into pages of `MaxResults`.
//!
//! The catalog is SERVICE data — the same answer for every caller, independent of
//! anything a caller created — so it lives in [`crate::state::RELEASE_LABELS`] and
//! this reporting read touches no store and changes nothing a later caller sees.
//!
//! ---
//!
//! # Modeled contract for `ListReleaseLabels`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json")
//! ```
//!
//! Retrieves release labels of Amazon EMR services in the Region where the API is called.
//!
//! ## Input
//!
//! * `Filters` (ReleaseLabelFilter) — Filters the results of the request. Prefix specifies
//!   the prefix of release labels to return. Application specifies the application
//!   (with/without version) of release labels to return.
//! * `MaxResults` (MaxResultsNumber, range 1..100) — Defines the maximum number of release
//!   labels to return in a single response. The default is 100.
//! * `NextToken` (String) — Specifies the next page of results. If NextToken is not
//!   specified, which is usually the case for the first request of ListReleaseLabels, the
//!   first page of results are determined by other filtering parameters or by the latest
//!   version. The ListReleaseLabels request fails if the identity (Amazon Web Services
//!   account ID) and all filtering parameters are different from the original request, or if
//!   the NextToken is expired or tampered with.
//!
//! ## Output
//!
//! * `NextToken` (String) — Used to paginate the next page of results if specified in the
//!   next ListReleaseLabels request.
//! * `ReleaseLabels` (StringList) — The returned release labels.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — This exception occurs when there is an internal
//!   failure in the Amazon EMR service.
//! * `InvalidRequestException` (HTTP 400) — This exception occurs when there is something
//!   wrong with user input.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListReleaseLabelsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-emr-elasticmapreduce.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListReleaseLabelsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.emr#ListReleaseLabels")]
#[derive(Debug)]
pub enum ListReleaseLabelsErrorModeled {
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_release_labels::ListReleaseLabelsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListReleaseLabelsError = crate::scaffolded::Scaffolded<ListReleaseLabelsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListReleaseLabelsErrorModeled> for ListReleaseLabelsError {
    fn from(modeled: ListReleaseLabelsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListReleaseLabels` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListReleaseLabels<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListReleaseLabels<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListReleaseLabels<Eng>
{
    type Input = ListReleaseLabelsInput;
    type Output = ListReleaseLabelsOutput;
    type Error = ListReleaseLabelsError;

    const NAME: &'static str = "ListReleaseLabels";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RELEASE_LABELS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RELEASE_LABELS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::LIST_RELEASE_LABELS_MAX_RESULTS;

        // The release catalog is service data: no request metadata and no store
        // entry takes part in the answer, and nothing here writes.
        let _ = (ctx, &self.engine);

        let filters = input.filters.unwrap_or_default();
        let prefix = filters.prefix.as_deref();
        let application = filters.application.as_deref();

        // Newest release first, then narrowed by each filter the caller supplied:
        // `Prefix` keeps the labels that start with it ("Optional release label
        // version prefix filter. For example, `emr-5`."), `Application` keeps the
        // releases that bundle the named application, at the named version when the
        // filter carries one.
        let matched: Vec<String> = crate::state::catalog_release_labels()
            .into_iter()
            .filter(|label| prefix.is_none_or(|prefix| label.starts_with(prefix)))
            .filter(|label| {
                application
                    .is_none_or(|filter| crate::state::release_bundles_application(label, filter))
            })
            .collect();

        // A page token is bound to the filters it was minted for, so resuming with
        // a different filter pair — or with a token this listing never minted — is
        // refused rather than answered from the wrong list.
        let fingerprint = crate::state::release_label_filter_fingerprint(prefix, application);
        let start = match input.next_token.as_deref() {
            Some(token) => {
                let resume_after = crate::state::decode_release_label_token(token, &fingerprint)
                    .ok()
                    .and_then(|cursor| matched.iter().position(|label| *label == cursor));
                match resume_after {
                    Some(at) => at + 1,
                    None => {
                        return Err(ListReleaseLabelsErrorModeled::InvalidRequestException(
                            InvalidRequestException {
                                error_code: None,
                                message: Some(
                                    "NextToken is not valid for this request's filters."
                                        .to_string(),
                                ),
                            },
                        )
                        .into())
                    }
                }
            }
            None => 0,
        };

        let page_size = input
            .max_results
            .unwrap_or(LIST_RELEASE_LABELS_MAX_RESULTS)
            .clamp(1, LIST_RELEASE_LABELS_MAX_RESULTS) as usize;
        let end = matched.len().min(start.saturating_add(page_size));
        let release_labels: Vec<String> = matched[start.min(end)..end].to_vec();

        // A token only where a release is left to report, carrying the last label
        // this page served: the next page resumes after it.
        let next_token = (end < matched.len() && end > start).then(|| {
            crate::state::encode_release_label_token(&fingerprint, &matched[end - 1])
        });

        Ok(ListReleaseLabelsOutput {
            release_labels: Some(release_labels),
            next_token,
        })
    }
}
